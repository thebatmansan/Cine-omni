//! The editorial workspace: docked panels around a real picture.
//!
//! ## Why this is native and not the browser
//!
//! A decoded frame has to reach the screen without leaving the GPU. The
//! measured cost of the alternative is in `docs/blueprint/PLAYBACK.md`: reading
//! a converted frame back to the CPU and uploading it again cost 69.4 ms per
//! picture against a 41.7 ms budget, and dropped 77% of them. A webview adds
//! another copy on top of that. The browser UI remains the review surface; the
//! cutting room is here.
//!
//! ## What this module is, and is not
//!
//! It is the panel layout, the drawing, and the *intent* the interface
//! produces — a [`Command`]. It owns no playback state, holds no scheduler and
//! decodes nothing: it is handed a [`ViewerStatus`] describing what is true and
//! returns what the editor asked for. That is what makes the transport testable
//! without a window, and what stops the interface from becoming a second place
//! where playback state lives.
//!
//! The picture is **not** drawn here. [`Workspace::program_rect`] reports where
//! the program viewer ended up and the caller draws the frame into it with the
//! verified planar path, under the panels. Drawing it here would mean either a
//! second presentation path or an egui texture round-trip — the exact cost this
//! architecture exists to avoid.

use crate::{Availability, ShellCapabilities};
use cine_media::clock::TransportState;
use cine_timeline::session::TrackState;
use cine_timeline::{Composable, Timeline, TrackKind};
use egui::{Align2, Color32, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::time::Duration;
use ucg_core::RationalTime;

/// What the editor asked the application to do.
///
/// Returned rather than performed: the workspace has no transport to drive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Play,
    Pause,
    /// Play if stopped, pause if playing. What the space bar means.
    TogglePlay,
    PlayReverse,
    Stop,
    Step(i64),
    /// Move the playhead to roughly this time.
    ///
    /// Approximate on purpose: a click lands between pictures, and only the
    /// source knows where the pictures are. The caller resolves it to an exact
    /// presentation time — this asset's first picture is at 1024/12288, not
    /// zero, so a workspace that computed an index from seconds itself would be
    /// wrong by two frames before anything else went wrong.
    SeekNear(RationalTime),
    ToggleLock(usize),
    ToggleMute(usize),
    ToggleSolo(usize),
    ToggleTarget(usize),
}

/// Everything the interface needs to know, and nothing it could get wrong.
///
/// Assembled by the caller from the scheduler and the decoder each frame, so
/// there is exactly one source of truth for every number shown.
#[derive(Clone, Debug, Default)]
pub struct ViewerStatus {
    pub media: Option<String>,
    pub state: Option<TransportState>,
    pub position: Option<RationalTime>,
    pub picture: Option<usize>,
    pub pictures: usize,
    pub presented: u64,
    pub dropped: u64,
    pub late: u64,
    pub underrun_episodes: u64,
    pub decode_mean: Option<Duration>,
    pub buffered: usize,
    pub buffer_bytes: usize,
    /// Colour interpretation and whether it had to be assumed.
    pub colour: Option<(String, bool)>,
    /// Scopes of the picture on screen, when one has been measured.
    pub scopes: Option<std::sync::Arc<crate::scopes::Scopes>>,
    /// The view the picture is shown through, and how far the lattice the GPU
    /// interpolates is from the transform itself, in code value of 255.
    pub view: Option<(String, f32)>,
    /// Why the picture on screen is black, when it is: a gap, or media that
    /// cannot be played. Said over the picture, never left to be guessed.
    pub note: Option<String>,
    pub keyframes: Vec<usize>,
}

impl ViewerStatus {
    fn transport_label(&self) -> &str {
        match self.state {
            Some(s) => s.label(),
            None => "no media",
        }
    }
}

/// A docked panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Program,
    Timeline,
    Project,
    Inspector,
    Capabilities,
    Scopes,
}

impl Pane {
    fn title(&self) -> &'static str {
        match self {
            Pane::Program => "Program",
            Pane::Timeline => "Timeline",
            Pane::Project => "Project",
            Pane::Inspector => "Inspector",
            Pane::Capabilities => "Capabilities",
            Pane::Scopes => "Scopes",
        }
    }
}

// A neutral, dense palette. A cutting room is dark because the picture is the
// only thing that should be bright.
const BG: Color32 = Color32::from_rgb(24, 24, 26);
const PANEL: Color32 = Color32::from_rgb(32, 32, 35);
const LINE: Color32 = Color32::from_rgb(58, 58, 63);
const TEXT: Color32 = Color32::from_rgb(214, 214, 218);
const DIM: Color32 = Color32::from_rgb(140, 140, 148);
const VIDEO_CLIP: Color32 = Color32::from_rgb(60, 88, 120);
const AUDIO_CLIP: Color32 = Color32::from_rgb(58, 104, 82);
const PLAYHEAD: Color32 = Color32::from_rgb(232, 96, 72);
const WARN: Color32 = Color32::from_rgb(224, 160, 64);
const OK: Color32 = Color32::from_rgb(120, 190, 130);

/// A track-header toggle: its label, whether it is on, and what clicking means.
type TrackFlag = (&'static str, bool, fn(usize) -> Command);

const HEADER_W: f32 = 148.0;
const TRACK_H: f32 = 44.0;
const RULER_H: f32 = 22.0;

/// The workspace: a docked layout, the timeline, and the transport.
pub struct Workspace {
    tree: egui_tiles::Tree<Pane>,
    program_rect: Option<Rect>,
    /// Seconds of timeline shown per screen point.
    zoom: f32,
    scroll: f32,
    selected_track: Option<usize>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

impl Workspace {
    pub fn new() -> Workspace {
        let mut tiles = egui_tiles::Tiles::default();
        let program = tiles.insert_pane(Pane::Program);
        let project = tiles.insert_pane(Pane::Project);
        let inspector = tiles.insert_pane(Pane::Inspector);
        let capabilities = tiles.insert_pane(Pane::Capabilities);
        let timeline = tiles.insert_pane(Pane::Timeline);

        let left = tiles.insert_tab_tile(vec![project]);
        let scopes = tiles.insert_pane(Pane::Scopes);
        let right = tiles.insert_tab_tile(vec![inspector, scopes, capabilities]);
        let top = tiles.insert_horizontal_tile(vec![left, program, right]);
        let root = tiles.insert_vertical_tile(vec![top, timeline]);
        Workspace {
            tree: egui_tiles::Tree::new("cine-workspace", root, tiles),
            program_rect: None,
            zoom: 24.0,
            scroll: 0.0,
            selected_track: None,
        }
    }

    /// Where the program viewer ended up last frame, in physical pixels.
    ///
    /// `None` until it has been laid out, or while it is not visible.
    pub fn program_rect(&self) -> Option<Rect> {
        self.program_rect
    }

    /// Draw the whole workspace. Returns what the editor asked for.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        status: &ViewerStatus,
        timeline: Option<&Timeline>,
    ) -> Vec<Command> {
        let mut commands = Vec::new();
        {
            let visuals = &mut ui.style_mut().visuals;
            visuals.panel_fill = PANEL;
            visuals.window_fill = PANEL;
            visuals.override_text_color = Some(TEXT);
        }

        egui::Panel::top("transport")
            .exact_size(38.0)
            .resizable(false)
            .show(ui, |ui| {
                self.transport_bar(ui, status, &mut commands);
            });

        let mut behavior = PaneBehavior {
            status,
            timeline,
            commands: &mut commands,
            program_rect: None,
            zoom: &mut self.zoom,
            scroll: &mut self.scroll,
            selected_track: &mut self.selected_track,
        };
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BG))
            .show(ui, |ui| {
                self.tree.ui(&mut behavior, ui);
            });
        self.program_rect = behavior.program_rect;

        commands.extend(keyboard(ui.ctx()));
        commands
    }

    fn transport_bar(&mut self, ui: &mut Ui, status: &ViewerStatus, out: &mut Vec<Command>) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let playing = status.state.map(|s| s.is_advancing()).unwrap_or(false);
            if ui.button("⏮").on_hover_text("Stop (Home)").clicked() {
                out.push(Command::Stop);
            }
            if ui.button("◀◀").on_hover_text("Reverse (J)").clicked() {
                out.push(Command::PlayReverse);
            }
            if ui.button("◀").on_hover_text("Step back (←)").clicked() {
                out.push(Command::Step(-1));
            }
            let play_label = if playing { "❚❚" } else { "▶" };
            if ui
                .button(play_label)
                .on_hover_text("Play / pause (Space)")
                .clicked()
            {
                out.push(if playing {
                    Command::Pause
                } else {
                    Command::Play
                });
            }
            if ui.button("▶").on_hover_text("Step forward (→)").clicked() {
                out.push(Command::Step(1));
            }

            ui.add_space(12.0);
            let timecode = status
                .position
                .map(timecode_of)
                .unwrap_or_else(|| "--:--:--:--".into());
            ui.label(
                RichText::new(timecode)
                    .font(FontId::monospace(16.0))
                    .color(TEXT),
            );
            if let (Some(i), n) = (status.picture, status.pictures) {
                ui.label(
                    RichText::new(format!("{i} / {n}"))
                        .font(FontId::monospace(12.0))
                        .color(DIM),
                );
            }

            ui.add_space(12.0);
            let (label, colour) = transport_colour(status);
            ui.label(RichText::new(label).color(colour).strong());

            if status.dropped > 0 || status.underrun_episodes > 0 {
                ui.label(
                    RichText::new(format!(
                        "{} dropped · {} stalls",
                        status.dropped, status.underrun_episodes
                    ))
                    .font(FontId::monospace(12.0))
                    .color(WARN),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(mean) = status.decode_mean {
                    ui.label(
                        RichText::new(format!("decode {:.1} ms", mean.as_secs_f64() * 1e3))
                            .font(FontId::monospace(12.0))
                            .color(DIM),
                    );
                }
                if status.buffer_bytes > 0 {
                    ui.label(
                        RichText::new(format!(
                            "buffer {} · {:.0} MiB",
                            status.buffered,
                            status.buffer_bytes as f64 / (1024.0 * 1024.0)
                        ))
                        .font(FontId::monospace(12.0))
                        .color(DIM),
                    );
                }
            });
        });
    }
}

fn transport_colour(status: &ViewerStatus) -> (String, Color32) {
    let label = status.transport_label().to_uppercase();
    let colour = match status.state {
        Some(TransportState::Playing) => OK,
        Some(TransportState::Degraded) | Some(TransportState::Buffering) => WARN,
        Some(TransportState::Unsupported) => PLAYHEAD,
        _ => DIM,
    };
    (label, colour)
}

/// Keyboard, in the shape an editor expects: J/K/L, space, arrows.
fn keyboard(ctx: &egui::Context) -> Vec<Command> {
    let mut out = Vec::new();
    ctx.input(|i| {
        if i.key_pressed(egui::Key::Space) {
            out.push(Command::TogglePlay);
        }
        if i.key_pressed(egui::Key::J) {
            out.push(Command::PlayReverse);
        }
        if i.key_pressed(egui::Key::K) {
            out.push(Command::Pause);
        }
        if i.key_pressed(egui::Key::L) {
            out.push(Command::Play);
        }
        if i.key_pressed(egui::Key::ArrowRight) {
            out.push(Command::Step(1));
        }
        if i.key_pressed(egui::Key::ArrowLeft) {
            out.push(Command::Step(-1));
        }
        if i.key_pressed(egui::Key::Home) {
            out.push(Command::Stop);
        }
    });
    out
}

/// SMPTE timecode, non-drop, from exact time. Display only.
fn timecode_of(t: RationalTime) -> String {
    t.to_timecode()
}

struct PaneBehavior<'a> {
    status: &'a ViewerStatus,
    timeline: Option<&'a Timeline>,
    commands: &'a mut Vec<Command>,
    program_rect: Option<Rect>,
    zoom: &'a mut f32,
    scroll: &'a mut f32,
    selected_track: &'a mut Option<usize>,
}

impl egui_tiles::Behavior<Pane> for PaneBehavior<'_> {
    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        RichText::new(pane.title()).color(TEXT).into()
    }

    fn pane_ui(
        &mut self,
        ui: &mut Ui,
        _tile: egui_tiles::TileId,
        pane: &mut Pane,
    ) -> egui_tiles::UiResponse {
        match pane {
            Pane::Program => self.program(ui),
            Pane::Timeline => self.timeline_pane(ui),
            Pane::Project => self.project(ui),
            Pane::Inspector => self.inspector(ui),
            Pane::Capabilities => self.capabilities(ui),
            Pane::Scopes => self.scopes(ui),
        }
        egui_tiles::UiResponse::None
    }
}

impl PaneBehavior<'_> {
    /// The program viewer draws nothing: it reserves the space and reports it,
    /// and the caller puts the real picture there.
    fn program(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
        self.program_rect = Some(rect);

        if self.status.media.is_none() {
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                "no media open",
                FontId::proportional(14.0),
                DIM,
            );
        }
        if let Some(note) = &self.status.note {
            ui.painter().text(
                rect.center_top() + Vec2::new(0.0, 16.0),
                Align2::CENTER_TOP,
                note,
                FontId::proportional(13.0),
                WARN,
            );
        }
        if let Some((note, assumed)) = &self.status.colour {
            let colour = if *assumed { WARN } else { DIM };
            ui.painter().text(
                rect.left_bottom() + Vec2::new(8.0, -8.0),
                Align2::LEFT_BOTTOM,
                note,
                FontId::monospace(11.0),
                colour,
            );
        }
        // What the picture is being shown through, said on the picture: a
        // viewer that transforms silently is one nobody can trust.
        let view = match &self.status.view {
            Some((what, error)) => format!("{what}  ±{:.1}/255", error * 255.0),
            None => "no view transform — shown as decoded".to_string(),
        };
        ui.painter().text(
            rect.right_bottom() + Vec2::new(-8.0, -8.0),
            Align2::RIGHT_BOTTOM,
            view,
            FontId::monospace(11.0),
            DIM,
        );
        ui.allocate_rect(rect, Sense::hover());
    }

    fn project(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        match &self.status.media {
            Some(path) => {
                ui.label(RichText::new("Media").color(DIM).size(11.0));
                ui.label(RichText::new(path).font(FontId::monospace(11.0)));
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("{} pictures", self.status.pictures))
                        .font(FontId::monospace(11.0))
                        .color(DIM),
                );
                if !self.status.keyframes.is_empty() {
                    ui.label(
                        RichText::new(format!(
                            "{} keyframes, longest GOP {}",
                            self.status.keyframes.len(),
                            longest_gop(&self.status.keyframes, self.status.pictures)
                        ))
                        .font(FontId::monospace(11.0))
                        .color(DIM),
                    );
                }
            }
            None => {
                ui.label(RichText::new("No media open.").color(DIM));
                ui.label(
                    RichText::new("cine-shell <file>")
                        .font(FontId::monospace(11.0))
                        .color(DIM),
                );
            }
        }
    }

    fn inspector(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        let s = self.status;
        let row = |ui: &mut Ui, k: &str, v: String| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(k).color(DIM).size(11.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(v).font(FontId::monospace(11.0)));
                });
            });
        };
        row(ui, "transport", s.transport_label().into());
        row(
            ui,
            "position",
            s.position.map(timecode_of).unwrap_or_else(|| "—".into()),
        );
        row(ui, "presented", s.presented.to_string());
        row(ui, "dropped", s.dropped.to_string());
        row(ui, "late", s.late.to_string());
        row(ui, "stalls", s.underrun_episodes.to_string());
        if let Some(mean) = s.decode_mean {
            row(
                ui,
                "decode mean",
                format!("{:.2} ms", mean.as_secs_f64() * 1e3),
            );
        }
        if let Some(track) = *self.selected_track {
            ui.separator();
            row(ui, "selected track", track.to_string());
        }
    }

    /// Capability reporting, straight from `ShellCapabilities`. A panel that
    /// says what does not work is the one panel that must never be omitted.
    fn capabilities(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        for (name, availability) in ShellCapabilities::all() {
            let (mark, colour) = match availability {
                Availability::Working => ("●", OK),
                Availability::Degraded(_) => ("◐", WARN),
                Availability::Unavailable(_) => ("○", DIM),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(mark).color(colour));
                ui.label(RichText::new(name).size(12.0));
            });
            if let Some(why) = availability.reason() {
                ui.label(RichText::new(why).size(10.0).color(DIM).italics());
            }
            ui.add_space(4.0);
        }
    }

    /// Waveform, parade and vectorscope of the picture on screen.
    ///
    /// Drawn from the counts `cine-color` produced, with no smoothing and no
    /// gain: a trace is where the pixels are. The heading says what was
    /// measured — how much of the picture, and whether it went through the
    /// view — because a scope that does not say what it read is a decoration.
    fn scopes(&mut self, ui: &mut Ui) {
        let Some(scopes) = self.status.scopes.clone() else {
            ui.add_space(6.0);
            ui.label(
                RichText::new("No picture measured yet.")
                    .size(11.0)
                    .color(DIM),
            );
            return;
        };
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!(
                "{} samples · {:.1} ms · {}",
                scopes.sampled,
                scopes.took.as_secs_f64() * 1000.0,
                if scopes.through_view {
                    "through the view"
                } else {
                    "as decoded"
                }
            ))
            .size(10.0)
            .color(DIM),
        );
        let width = ui.available_width().max(32.0);
        let height = (ui.available_height() - 24.0).max(60.0);
        let each = (height / 2.0 - 6.0).max(40.0);

        let waveform = ui.allocate_space(Vec2::new(width, each)).1;
        draw_waveform(ui, waveform, &scopes.waveform);
        ui.add_space(4.0);
        let vector = ui.allocate_space(Vec2::new(width, each)).1;
        draw_vectorscope(ui, vector, &scopes.vectorscope);
    }

    fn timeline_pane(&mut self, ui: &mut Ui) {
        // Clipped to something finite on purpose. `available_rect_before_wrap`
        // can be unbounded, and the ruler walks ticks until it passes the right
        // edge — with an infinite edge that loop never ends. It hung the whole
        // application, and only once a timeline was loaded, because the empty
        // case returned before reaching the ruler.
        let full = ui.available_rect_before_wrap().intersect(ui.clip_rect());
        if !full.width().is_finite() || !full.height().is_finite() || full.width() < 1.0 {
            return;
        }
        ui.painter().rect_filled(full, 0.0, PANEL);
        let Some(timeline) = self.timeline else {
            ui.painter().text(
                full.center(),
                Align2::CENTER_CENTER,
                "no timeline loaded",
                FontId::proportional(13.0),
                DIM,
            );
            ui.allocate_rect(full, Sense::hover());
            return;
        };

        let response = ui.allocate_rect(full, Sense::click_and_drag());
        // Zoom on scroll, because a timeline that cannot be zoomed is a picture
        // of a timeline.
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if response.hovered() && scroll != 0.0 {
            *self.zoom = (*self.zoom * (1.0 + scroll * 0.002)).clamp(2.0, 2000.0);
        }

        let painter = ui.painter_at(full);
        let body = Rect::from_min_max(full.min + Vec2::new(0.0, RULER_H), full.max);
        let track_area = Rect::from_min_max(body.min + Vec2::new(HEADER_W, 0.0), body.max);

        self.ruler(&painter, full, track_area);

        let tracks: Vec<&cine_timeline::Track> = timeline
            .tracks
            .children
            .iter()
            .filter_map(Composable::as_track)
            .collect();

        for (index, track) in tracks.iter().enumerate() {
            let top = body.min.y + index as f32 * TRACK_H;
            if top > body.max.y {
                break;
            }
            let row = Rect::from_min_size(
                egui::pos2(full.min.x, top),
                Vec2::new(full.width(), TRACK_H),
            );
            self.track_row(ui, &painter, row, track_area, index, track);
        }

        // The playhead, over everything.
        if let Some(position) = self.status.position {
            let x = track_area.min.x + self.x_of(position);
            if track_area.x_range().contains(x) {
                painter.line_segment(
                    [egui::pos2(x, full.min.y), egui::pos2(x, full.max.y)],
                    Stroke::new(1.5, PLAYHEAD),
                );
            }
        }

        // Clicking in the track area moves the playhead.
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                if pos.x > track_area.min.x {
                    let seconds = (pos.x - track_area.min.x) / *self.zoom + *self.scroll;
                    let rate = self
                        .status
                        .position
                        .map(|p| p.rate_exact())
                        .unwrap_or(ucg_core::Rate::F24);
                    self.commands
                        .push(Command::SeekNear(RationalTime::from_seconds(
                            f64::from(seconds.max(0.0)),
                            rate.as_f64(),
                        )));
                }
            }
        }
    }

    fn ruler(&self, painter: &egui::Painter, full: Rect, track_area: Rect) {
        let rect = Rect::from_min_size(full.min, Vec2::new(full.width(), RULER_H));
        painter.rect_filled(rect, 0.0, Color32::from_rgb(28, 28, 31));
        painter.line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(1.0, LINE),
        );
        // A tick every second, labelled where there is room. The iteration cap
        // is not decoration: a non-finite width here once hung the application,
        // and a drawing loop should fail to draw rather than fail to return.
        let step = tick_seconds(*self.zoom).max(f32::MIN_POSITIVE);
        let width = track_area.width();
        if !width.is_finite() {
            return;
        }
        let mut ticks = 0;
        let mut second = (*self.scroll / step).floor() * step;
        while (second - *self.scroll) * *self.zoom < width && ticks < 4096 {
            ticks += 1;
            let x = track_area.min.x + (second - *self.scroll) * *self.zoom;
            if x >= track_area.min.x {
                painter.line_segment(
                    [egui::pos2(x, rect.max.y - 6.0), egui::pos2(x, rect.max.y)],
                    Stroke::new(1.0, LINE),
                );
                painter.text(
                    egui::pos2(x + 3.0, rect.min.y + 3.0),
                    Align2::LEFT_TOP,
                    clock_label(second),
                    FontId::monospace(10.0),
                    DIM,
                );
            }
            second += step;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn track_row(
        &mut self,
        ui: &mut Ui,
        painter: &egui::Painter,
        row: Rect,
        track_area: Rect,
        index: usize,
        track: &cine_timeline::Track,
    ) {
        let header = Rect::from_min_size(row.min, Vec2::new(HEADER_W, row.height()));
        painter.rect_filled(header, 0.0, Color32::from_rgb(28, 28, 31));
        painter.line_segment(
            [row.left_bottom(), row.right_bottom()],
            Stroke::new(1.0, LINE),
        );
        painter.text(
            header.min + Vec2::new(8.0, 6.0),
            Align2::LEFT_TOP,
            &track.name,
            FontId::proportional(12.0),
            if track.enabled { TEXT } else { DIM },
        );

        // Lock, mute, solo, target — the state the editor actually works in.
        let state = TrackState::of(track);
        let flags: [TrackFlag; 4] = [
            ("L", state.locked, Command::ToggleLock),
            ("M", state.muted, Command::ToggleMute),
            ("S", state.solo, Command::ToggleSolo),
            ("T", state.targeted, Command::ToggleTarget),
        ];
        for (i, (label, on, make)) in flags.iter().enumerate() {
            let at = header.min + Vec2::new(8.0 + i as f32 * 20.0, 24.0);
            let button = Rect::from_min_size(at, Vec2::new(17.0, 15.0));
            let response = ui.interact(button, ui.id().with(("flag", index, i)), Sense::click());
            let fill = if *on {
                WARN
            } else {
                Color32::from_rgb(44, 44, 48)
            };
            painter.rect_filled(button, 2.0, fill);
            painter.text(
                button.center(),
                Align2::CENTER_CENTER,
                *label,
                FontId::monospace(10.0),
                if *on { Color32::BLACK } else { DIM },
            );
            if response.clicked() {
                self.commands.push(make(index));
            }
        }

        // Clips.
        let base = if track.kind == TrackKind::Audio {
            AUDIO_CLIP
        } else {
            VIDEO_CLIP
        };
        for child in 0..track.children.len() {
            let Some(range) = track.range_of_child(child) else {
                continue;
            };
            let x0 = track_area.min.x + self.x_of(range.start_time);
            let x1 = track_area.min.x + self.x_of(range.end_time_exclusive());
            if x1 < track_area.min.x || x0 > track_area.max.x {
                continue;
            }
            let clip = Rect::from_min_max(
                egui::pos2(x0.max(track_area.min.x), row.min.y + 3.0),
                egui::pos2(x1.min(track_area.max.x), row.max.y - 3.0),
            );
            if clip.width() < 1.0 {
                continue;
            }
            let fill = if track.enabled {
                base
            } else {
                Color32::from_rgb(52, 52, 56)
            };
            painter.rect_filled(clip, 2.0, fill);
            painter.rect_stroke(clip, 2.0, Stroke::new(1.0, LINE), StrokeKind::Inside);
            if clip.width() > 28.0 {
                let name = track.children[child].name();
                painter.text(
                    clip.min + Vec2::new(4.0, 3.0),
                    Align2::LEFT_TOP,
                    name,
                    FontId::proportional(11.0),
                    TEXT,
                );
            }
        }
    }

    /// Horizontal offset of a time, in points.
    fn x_of(&self, t: RationalTime) -> f32 {
        (t.to_seconds() as f32 - *self.scroll) * *self.zoom
    }
}

/// Seconds between ruler ticks, so labels never collide.
fn tick_seconds(zoom: f32) -> f32 {
    const STEPS: [f32; 10] = [0.04, 0.2, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 300.0, 600.0];
    *STEPS.iter().find(|s| **s * zoom >= 56.0).unwrap_or(&600.0)
}

fn clock_label(seconds: f32) -> String {
    let total = seconds.max(0.0) as i64;
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        (total / 60) % 60,
        total % 60
    )
}

fn longest_gop(keyframes: &[usize], count: usize) -> usize {
    keyframes
        .windows(2)
        .map(|w| w[1] - w[0])
        .chain(keyframes.last().map(|&k| count.saturating_sub(k)))
        .max()
        .unwrap_or(count)
}

/// A luma waveform: one column per sampled column, brightness by count.
///
/// Counts are drawn on a square-root curve, which is what a scope does: a
/// single stray pixel must be visible against a flat sky without the flat sky
/// saturating everything.
fn draw_waveform(ui: &Ui, rect: Rect, wf: &cine_color::scopes::Waveform) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(12, 12, 14));
    let peak = wf.luma.iter().copied().max().unwrap_or(1).max(1) as f32;
    let cw = rect.width() / wf.columns.max(1) as f32;
    let rh = rect.height() / wf.rows.max(1) as f32;
    for col in 0..wf.columns {
        for row in 0..wf.rows {
            let n = cine_color::scopes::Waveform::at(&wf.luma, wf.rows, col, row) as f32;
            if n <= 0.0 {
                continue;
            }
            let a = (n / peak).sqrt().clamp(0.0, 1.0);
            // Row 0 is the bottom of the scale.
            let y = rect.bottom() - (row as f32 + 1.0) * rh;
            painter.rect_filled(
                Rect::from_min_size(
                    egui::pos2(rect.left() + col as f32 * cw, y),
                    Vec2::new(cw.max(1.0), rh.max(1.0)),
                ),
                0.0,
                Color32::from_rgba_unmultiplied(120, 235, 140, (a * 255.0) as u8),
            );
        }
    }
    // Reference lines at 0, 50 and 100% of the scale.
    for t in [0.0, 0.5, 1.0] {
        let y = rect.bottom() - t * rect.height();
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(90, 90, 100, 90)),
        );
    }
}

/// A vectorscope: hue around, saturation outward, with the primary targets.
fn draw_vectorscope(ui: &Ui, rect: Rect, vs: &cine_color::scopes::Vectorscope) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(12, 12, 14));
    let side = rect.width().min(rect.height());
    let centre = rect.center();
    let radius = side / 2.0 - 4.0;
    painter.circle_stroke(
        centre,
        radius,
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(90, 90, 100, 110)),
    );
    let peak = vs.density.iter().copied().max().unwrap_or(1).max(1) as f32;
    let n = vs.size.max(1);
    for y in 0..n {
        for x in 0..n {
            let count = vs.density[y * n + x] as f32;
            if count <= 0.0 {
                continue;
            }
            let a = (count / peak).sqrt().clamp(0.0, 1.0);
            // Bin centres run -0.5..0.5 in Cb/Cr; up is +Cr.
            let cb = (x as f32 + 0.5) / n as f32 - 0.5;
            let cr = (y as f32 + 0.5) / n as f32 - 0.5;
            let p = egui::pos2(centre.x + cb * 2.0 * radius, centre.y + cr * 2.0 * radius);
            painter.rect_filled(
                Rect::from_center_size(p, Vec2::splat((side / n as f32).max(1.0))),
                0.0,
                Color32::from_rgba_unmultiplied(235, 220, 120, (a * 255.0) as u8),
            );
        }
    }
    for target in &vs.targets {
        let p = egui::pos2(
            centre.x + target.x * 2.0 * radius,
            centre.y + target.y * 2.0 * radius,
        );
        painter.circle_stroke(p, 4.0, Stroke::new(1.0, DIM));
        painter.text(
            p + Vec2::new(6.0, -2.0),
            Align2::LEFT_CENTER,
            &target.name,
            FontId::monospace(9.0),
            DIM,
        );
    }
}
