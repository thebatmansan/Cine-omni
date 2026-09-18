//! Rendering a sequence: the delivery path.
//!
//! A render is the cut as the record monitor shows it, written to disk. Every
//! record frame is resolved through [`crate::playlist`] — the same edit list and
//! the same picture arithmetic the monitor and the native player use, retimes
//! and active angles included — then:
//!
//! ```text
//! decode → [project view] → [fit to the output size] → [captions] → write
//! ```
//!
//! and written as PNG frames (8 or 16 bits per channel) or as a QuickTime
//! movie: H.264 picture and, when the cut has sound, 24-bit PCM sound mixed by
//! the same mixer the player uses ([`cine_media::sequence_audio`]), retimes as
//! varispeed or with pitch kept, divided among pictures sample-exactly.
//!
//! ## What it refuses
//!
//! Nothing here renders a picture it cannot stand behind. Each of these refuses
//! the render and names the cause rather than producing something else:
//!
//! * a clip that is offline or undecodable, or a picture the decoder does not
//!   produce (never a black frame in its place);
//! * pictures of different sizes with no output size chosen, or a picture of a
//!   different shape from the output with no fit chosen (never a silent
//!   stretch or crop);
//! * a view asked for that some clip cannot be baked into (never the picture
//!   untransformed under a claimed view);
//! * sound asked for when the cut has none in that range (never a silent track
//!   presented as the cut's sound), a sound clip that cannot be played, or
//!   sound in PNG output;
//! * a movie through a display whose primaries this writer cannot declare
//!   (never a Rec.709 label on a P3 picture);
//! * captions in a language the sequence does not have, or a caption line wider
//!   than the safe area.
//!
//! A gap in the cut *is* black, and silence in the cut's sound *is* silence,
//! because that is what the cut says there.
//!
//! ## Precision
//!
//! Decoding is 8-bit (it is all this build decodes), but everything after it —
//! the view, the scaler, captions — works in 32-bit float, unclamped. What is
//! lost where is recorded: PNG 16 keeps a view's output to 1/65535; PNG 8 and
//! the movie quantise to 8 bits at the end, and the movie also to 4:2:0. The
//! manifest says which.
//!
//! ## Where it goes
//!
//! Renders are derived media (`docs/blueprint/STORAGE-POLICY.md`): retained,
//! never cleaned automatically. A render is written into `<dir>.partial` and
//! renamed into place only when every file and the manifest are written, so a
//! directory without `.partial` is complete. A cancelled or failed render's
//! partial directory is temporary data and is removed. The manifest records
//! what the render depended on ([`crate::provenance`]), so whether it is still
//! current can be answered later.

use crate::provenance::{self, Dependency};
use cine_comp::image::{ChannelSet, Image};
use cine_comp::ops::captions::{burn_in, CaptionFont, Placement as CaptionPlacement};
use cine_comp::ops::resample::{self, Fit, Placement};
use cine_media::encode::{H264Settings, YuvFrame};
use cine_media::mov::{ColourTags, MovSettings, MovWriter, SoundFormat};
use cine_media::sequence_audio::SequenceAudio;
use cine_media::AudioSource;
use cine_pipeline::OmniProject;
use serde_json::{json, Value as J};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use ucg_core::{EntityId, Rate, RationalTime, Rounding, TimeRange};

/// The job kind.
pub const RENDER_KIND: &str = "render.sequence";

/// The movie file inside a movie render's directory.
pub const MOVIE_FILE: &str = "movie.mov";

/// Sound in a movie: this rate, stereo, 24-bit.
pub const SOUND_RATE: u32 = 48_000;

/// How pictures are coloured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Look {
    /// The decoder's RGB, through no view: what the file holds.
    AsDecoded,
    /// Through a display and view of the project's colour config, with each
    /// clip's grade, exactly as the window shows it. `None` resolves, when the
    /// render runs, to the session's view or the config's default, and the
    /// resolved names are recorded so the render never changes meaning later.
    ProjectView {
        display: Option<String>,
        view: Option<String>,
    },
}

impl Look {
    pub fn as_str(&self) -> &'static str {
        match self {
            Look::AsDecoded => "as_decoded",
            Look::ProjectView { .. } => "project_view",
        }
    }
}

/// What a render writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Png8,
    Png16,
    /// QuickTime: H.264 picture, 24-bit PCM sound.
    Movie,
    /// OpenEXR frames, 32-bit float, unclamped: what scene-linear pictures are.
    Exr,
}

impl Output {
    pub fn parse(s: &str) -> Result<Output, String> {
        match s {
            "png8" | "png" => Ok(Output::Png8),
            "png16" => Ok(Output::Png16),
            "movie" => Ok(Output::Movie),
            "exr" => Ok(Output::Exr),
            other => Err(format!(
                "unknown output `{other}` (known: png8, png16, movie, exr)"
            )),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Output::Png8 => "png8",
            Output::Png16 => "png16",
            Output::Movie => "movie",
            Output::Exr => "exr",
        }
    }
}

/// What to render.
#[derive(Clone, Debug)]
pub struct RenderRequest {
    pub timeline: EntityId,
    /// The caption language to burn in, if any.
    pub captions: Option<String>,
    pub look: Look,
    /// Record frames `[start, end)` at the sequence rate; the whole cut when
    /// `None`. A render pins the frames it rendered here, and sets
    /// `whole_cut`, so its record says exactly which frames it holds.
    pub frames: Option<(i64, i64)>,
    /// Whether the frames were "the whole cut" when rendered: such a render
    /// is stale when the cut's length changes, even if those frames did not.
    pub whole_cut: bool,
    pub output: Output,
    /// For a movie: `Some(true)` requires sound, `Some(false)` leaves it out,
    /// `None` includes it when the cut has sound in the range.
    pub sound: Option<bool>,
    /// The output picture size; the sources' own size when `None`.
    pub size: Option<(usize, usize)>,
    pub fit: Fit,
    /// H.264 quantiser for a movie, 0 (best) to 51.
    pub quality: u8,
}

impl RenderRequest {
    pub fn from_params(params: &J) -> Result<RenderRequest, String> {
        let timeline = params
            .get("timeline")
            .and_then(J::as_str)
            .and_then(EntityId::parse)
            .ok_or("missing entity param `timeline`")?;
        let captions = params
            .get("captions")
            .and_then(J::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let text = |k: &str| params.get(k).and_then(J::as_str).map(str::to_string);
        let look = match params
            .get("look")
            .and_then(J::as_str)
            .unwrap_or("as_decoded")
        {
            "as_decoded" => Look::AsDecoded,
            "project_view" => Look::ProjectView {
                display: text("display"),
                view: text("view"),
            },
            other => {
                return Err(format!(
                    "unknown look `{other}` (known: as_decoded, project_view)"
                ))
            }
        };
        let frames = match (
            params.get("start_frame").and_then(J::as_i64),
            params.get("end_frame").and_then(J::as_i64),
        ) {
            (None, None) => None,
            (Some(s), Some(e)) if s >= 0 && e > s => Some((s, e)),
            _ => {
                return Err(
                    "`start_frame` and `end_frame` go together, with 0 ≤ start < end".into(),
                )
            }
        };
        let whole_cut = params
            .get("whole_cut")
            .and_then(J::as_bool)
            .unwrap_or(false);
        let output = Output::parse(params.get("output").and_then(J::as_str).unwrap_or("png8"))?;
        let sound = params.get("sound").and_then(J::as_bool);
        let size = match (
            params.get("width").and_then(J::as_u64),
            params.get("height").and_then(J::as_u64),
        ) {
            (None, None) => None,
            (Some(w), Some(h)) if w > 0 && h > 0 && w <= 16_384 && h <= 16_384 => {
                Some((w as usize, h as usize))
            }
            _ => return Err("`width` and `height` go together, each 1 to 16384".into()),
        };
        let fit = Fit::parse(params.get("fit").and_then(J::as_str).unwrap_or("refuse"))?;
        let quality = match params.get("quality").and_then(J::as_u64) {
            None => 18,
            Some(q) if q <= 51 => q as u8,
            Some(q) => return Err(format!("quality {q}: H.264 quantisers run 0 to 51")),
        };
        Ok(RenderRequest {
            timeline,
            captions,
            look,
            frames,
            whole_cut,
            output,
            sound,
            size,
            fit,
            quality,
        })
    }

    pub fn to_params(&self) -> J {
        let (display, view) = match &self.look {
            Look::ProjectView { display, view } => (display.clone(), view.clone()),
            Look::AsDecoded => (None, None),
        };
        json!({
            "timeline": self.timeline,
            "captions": self.captions,
            "look": self.look.as_str(),
            "display": display,
            "view": view,
            "start_frame": self.frames.map(|f| f.0),
            "end_frame": self.frames.map(|f| f.1),
            "whole_cut": self.whole_cut,
            "output": self.output.as_str(),
            "sound": self.sound,
            "width": self.size.map(|s| s.0),
            "height": self.size.map(|s| s.1),
            "fit": self.fit.as_str(),
            "quality": self.quality,
        })
    }
}

/// Where renders of a sequence live in a project directory.
pub fn render_root(root_dir: &Path, timeline: EntityId) -> PathBuf {
    root_dir
        .join("derived")
        .join("render")
        .join(timeline.to_string())
}

/// The file name of record frame `n`.
pub fn frame_file(n: i64) -> String {
    format!("frame_{n:06}.png")
}

/// One record frame, resolved: which entry and which picture of its file.
enum Planned {
    Black,
    Picture { entry: usize, index: usize },
}

/// Everything read from the graph for one render, gathered under one short
/// read so a long render never holds writers off.
struct Gathered {
    list: crate::playlist::EditList,
    plan: Vec<Planned>,
    first: i64,
    captions: Option<cine_timeline::captions::CaptionTrack>,
    views: Option<WantedViews>,
    sound: bool,
    revision: u64,
    request: RenderRequest,
    dependencies: Vec<Dependency>,
}

/// The record range `[first, last)` frames cover, at `rate`.
pub fn frame_range(rate: Rate, first: i64, last: i64) -> TimeRange {
    TimeRange::from_start_end(
        RationalTime::exact(first, rate),
        RationalTime::exact(last, rate),
    )
}

fn gather(project: &OmniProject, request: &RenderRequest) -> Result<Gathered, String> {
    let graph = project.engine.read();
    let node = graph
        .node(request.timeline)
        .ok_or("no such sequence in this project")?;
    if node.kind.as_str() != ucg_schema::kinds::NLE_TIMELINE {
        return Err(format!("{} is not a sequence", request.timeline));
    }
    let list = crate::playlist::edit_list(&graph, request.timeline)?;
    let total = list
        .end
        .rescale_checked(list.rate, Rounding::Ceil)
        .0
        .ticks();
    let (first, last) = request.frames.unwrap_or((0, total));
    if last > total {
        return Err(format!(
            "frame {last} is past the end of the cut ({total} frames)"
        ));
    }
    if first >= last {
        return Err("the cut has no frames to render".into());
    }
    let mut plan = Vec::with_capacity((last - first) as usize);
    for n in first..last {
        let at = RationalTime::exact(n, list.rate);
        let Some((i, entry)) = list
            .entries
            .iter()
            .enumerate()
            .find(|(_, e)| e.record.contains(at))
        else {
            plan.push(Planned::Black);
            continue;
        };
        if let Some(why) = entry.unplayable() {
            return Err(format!(
                "`{}` at frame {n} cannot be rendered: {why}",
                entry.name
            ));
        }
        let (_, index) = crate::playlist::picture_index(&graph, entry, at);
        let index = index.ok_or_else(|| {
            format!(
                "`{}` at frame {n}: its media declares no range, so which picture it shows cannot be known",
                entry.name
            )
        })?;
        plan.push(Planned::Picture {
            entry: i,
            index: index as usize,
        });
    }

    let range = frame_range(list.rate, first, last);
    let sound_here: Vec<&crate::playlist::Entry> = list
        .sound
        .iter()
        .filter(|e| e.record.overlaps(&range))
        .collect();
    let sound = match (request.output, request.sound) {
        (Output::Movie, Some(false)) => false,
        (Output::Movie, Some(true)) if sound_here.is_empty() => {
            return Err(
                "the cut has no sound in that range; render the movie without sound, or add sound first"
                    .into(),
            )
        }
        (Output::Movie, _) => !sound_here.is_empty(),
        (_, Some(true)) => {
            return Err("PNG frames carry no sound; render a movie to include it".into())
        }
        (_, _) => false,
    };
    if sound {
        if let Some(e) = sound_here.iter().find(|e| e.unplayable().is_some()) {
            return Err(format!(
                "the sound of `{}` cannot be played: {}; a movie without it would not be the cut's sound",
                e.name,
                e.unplayable().unwrap_or_default()
            ));
        }
    }

    let captions = match &request.captions {
        Some(language) => Some(
            cine_timeline::graph::export_captions(&graph, request.timeline, language)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("the sequence has no `{language}` captions to burn in"))?,
        ),
        None => None,
    };

    // Resolve the view and the frames now and pin them into the request, so
    // the manifest names what was rendered, and a later change of the
    // session's view or of the cut's length is judged against that.
    let mut request = request.clone();
    if request.frames.is_none() {
        request.frames = Some((first, last));
        request.whole_cut = true;
    }
    let views = match &request.look {
        Look::AsDecoded => None,
        Look::ProjectView { display, view } => {
            let w = wanted_views(project, &graph, &list, display.clone(), view.clone())?;
            request.look = Look::ProjectView {
                display: Some(w.display.clone()),
                view: Some(w.view.clone()),
            };
            Some(w)
        }
    };
    let dependencies = provenance::dependencies(project, &graph, &request, &list, sound)?;
    Ok(Gathered {
        list,
        plan,
        first,
        captions,
        views,
        sound,
        revision: project.engine.revision(),
        request,
        dependencies,
    })
}

/// Render `request` into `out`. `progress` is told how far along the render is
/// and returns `false` to cancel.
pub fn render_sequence(
    project: &OmniProject,
    request: &RenderRequest,
    out: &Path,
    progress: &mut dyn FnMut(f32, &str) -> bool,
) -> Result<J, String> {
    let began = Instant::now();
    let Gathered {
        list,
        plan,
        first,
        captions,
        views: view_wanted,
        sound,
        revision,
        request,
        dependencies,
    } = gather(project, request)?;

    let used: Vec<usize> = {
        let mut u: Vec<usize> = plan
            .iter()
            .filter_map(|s| match s {
                Planned::Picture { entry, .. } => Some(*entry),
                Planned::Black => None,
            })
            .collect();
        u.sort_unstable();
        u.dedup();
        u
    };
    // Bake each clip's view once, outside the graph read.
    let views = match &view_wanted {
        None => None,
        Some(w) => Some(resolve_views(w, &list, &used)?),
    };
    let movie_tags = match (request.output, &views) {
        (Output::Movie, Some(v)) => Some(display_tags(&v.display)?),
        _ => None,
    };

    // Open each file once, and learn each source's size.
    let mut readers: HashMap<PathBuf, cine_media::Pictures> = HashMap::new();
    let mut sizes: HashMap<usize, (usize, usize)> = HashMap::new();
    let mut declared: Vec<Option<(u8, u8)>> = Vec::new();
    for &i in &used {
        let e = &list.entries[i];
        let path = e.path.clone().ok_or("an entry has no file")?;
        if !readers.contains_key(&path) {
            let info = {
                let graph = project.engine.read();
                e.asset
                    .and_then(|a| cine_media::ucg::read_media(&graph, a))
                    .ok_or_else(|| format!("`{}` was never probed", e.name))?
            };
            let reader =
                cine_media::Pictures::open(info).map_err(|err| format!("`{}`: {err}", e.name))?;
            // Scene light has no display meaning until a view is applied.
            // Writing it into display-encoded output as decoded would clamp
            // every highlight to white, so that one combination refuses; the
            // same pictures render happily through a view, or as EXR.
            if reader.is_scene_float()
                && request.look == Look::AsDecoded
                && request.output != Output::Exr
            {
                return Err(format!(
                    "`{}` is scene-linear ({}): as decoded it has no display values, and writing \
                     it to {} would clamp every highlight to white. Render it through the project \
                     view, or as EXR, which keeps the scene values",
                    e.name,
                    reader.describe(),
                    request.output.as_str()
                ));
            }
            readers.insert(path.clone(), reader);
        }
        let reader = readers.get_mut(&path).expect("just opened");
        let video = reader
            .info()
            .streams
            .iter()
            .find_map(|s| s.video.as_ref())
            .ok_or_else(|| format!("`{}` has no pictures", e.name))?;
        sizes.insert(i, (video.width as usize, video.height as usize));
        if request.output == Output::Movie && views.is_none() {
            // What the source says its primaries and transfer are, from its
            // first picture, so a movie of it as decoded declares the same.
            let index = plan
                .iter()
                .find_map(|s| match s {
                    Planned::Picture { entry, index } if *entry == i => Some(*index),
                    _ => None,
                })
                .unwrap_or(0);
            let frame = reader
                .frame_at_index(index)
                .map_err(|err| format!("`{}` picture {index}: {err}", e.name))?
                .ok_or_else(|| format!("`{}` has no picture {index}", e.name))?;
            declared.push(frame.color.primaries.zip(frame.color.transfer));
        }
    }
    let Some(&first_size) = used.first().and_then(|i| sizes.get(i)) else {
        return Err("there is no picture in that part of the cut to render".into());
    };
    let (width, height) = match request.size {
        Some(s) => s,
        None => {
            if let Some((i, s)) = used
                .iter()
                .map(|i| (*i, sizes[i]))
                .find(|(_, s)| *s != first_size)
            {
                let name = &list.entries[used[0]].name;
                return Err(format!(
                    "`{}` is {}x{} but `{name}` is {}x{}: choose an output size and how pictures fit it",
                    list.entries[i].name, s.0, s.1, first_size.0, first_size.1
                ));
            }
            first_size
        }
    };
    let mut placements: HashMap<usize, Placement> = HashMap::new();
    for &i in &used {
        let (sw, sh) = sizes[&i];
        let p = resample::placement(sw, sh, width, height, request.fit)
            .map_err(|why| format!("`{}`: {why}", list.entries[i].name))?;
        placements.insert(i, p);
    }
    if request.output == Output::Movie && (!width.is_multiple_of(2) || !height.is_multiple_of(2)) {
        return Err(format!(
            "a {width}x{height} movie: H.264 4:2:0 needs an even width and height; choose an output size"
        ));
    }
    let colour = match (request.output, movie_tags) {
        (Output::Movie, Some(t)) => Some(t),
        (Output::Movie, None) => {
            let first = declared.first().copied().flatten();
            Some(match first {
                Some((p, t)) if declared.iter().all(|d| *d == Some((p, t))) => ColourTags {
                    primaries: u16::from(p),
                    transfer: u16::from(t),
                },
                _ => ColourTags::UNSPECIFIED,
            })
        }
        _ => None,
    };
    if request.output == Output::Exr && captions.is_some() {
        return Err(
            "captions cannot be burned into EXR: the burn-in draws display white, which is not a \
             scene value. Deliver the captions as a file, or render a display-encoded output"
                .into(),
        );
    }
    let font = match &captions {
        Some(_) => Some(CaptionFont::system().map_err(|e| e.to_string())?),
        None => None,
    };
    let mut audio = if sound {
        let mut a = SequenceAudio::new(
            Rate::whole(SOUND_RATE).expect("48 kHz"),
            list.end,
            crate::playlist::sound_entries(&list),
        );
        let refusals = a.open_all();
        if let Some((name, why)) = refusals.iter().next() {
            return Err(format!(
                "the sound of `{name}` cannot be mixed: {why}; a movie without it would not be the cut's sound"
            ));
        }
        Some(a)
    } else {
        None
    };

    let partial = partial_dir(out);
    let result = (|| -> Result<Option<J>, String> {
        std::fs::create_dir_all(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
        let mut burned = 0usize;
        let mut decoded_as: Vec<String> = Vec::new();
        let mut movie = match (request.output, colour) {
            (Output::Movie, Some(colour)) => Some(
                MovWriter::create(
                    &partial.join(MOVIE_FILE),
                    width,
                    height,
                    list.rate,
                    MovSettings {
                        h264: H264Settings {
                            qp: request.quality,
                            gop: 48,
                        },
                        sound: sound.then_some(SoundFormat {
                            sample_rate: SOUND_RATE,
                            channels: 2,
                        }),
                        colour,
                        // No record timecode: a sequence has no start
                        // timecode in this build yet, and writing one would
                        // be inventing the number a conform depends on.
                        timecode: None,
                        first_picture: first as u64,
                    },
                )
                .map_err(|e| e.to_string())?,
            ),
            _ => None,
        };
        let conversion = cine_gpu::yuv::YuvConversion {
            choice: cine_gpu::yuv::MatrixChoice::Declared {
                code: 1,
                matrix: cine_gpu::yuv::Matrix::BT709,
            },
            range: cine_gpu::yuv::Range::Limited,
            width: width as u32,
            height: height as u32,
            chroma_width: (width / 2) as u32,
            chroma_height: (height / 2) as u32,
        };
        let mut encode_clipped = 0u64;
        let mut sound_buf: Vec<f32> = Vec::new();
        let mut sound_pos: i64 = if sound {
            (i128::from(first) * i128::from(list.rate.den()) * i128::from(SOUND_RATE)
                / i128::from(list.rate.num())) as i64
        } else {
            0
        };
        let count = plan.len();
        for (k, step) in plan.iter().enumerate() {
            let n = first + k as i64;
            if !progress(k as f32 / count as f32, &format!("frame {n}")) {
                return Ok(None);
            }
            let image = match step {
                Planned::Black => {
                    let mut img = Image::rgba(width, height);
                    for a in img.data.as_chunks_mut::<4>().0 {
                        a[3] = 1.0;
                    }
                    img
                }
                Planned::Picture { entry, index } => {
                    let e = &list.entries[*entry];
                    let path = e.path.as_ref().ok_or("an entry has no file")?;
                    let reader = readers.get_mut(path).ok_or("a file was not opened")?;
                    let frame = reader
                        .frame_at_index(*index)
                        .map_err(|err| format!("`{}` picture {index}: {err}", e.name))?
                        .ok_or_else(|| {
                            format!(
                                "`{}` has no picture {index}: the cut asks for more of the file than it holds",
                                e.name
                            )
                        })?;
                    let (mut px, fw, fh, describe, assumed) = crate::frames::rgba_f32(&frame)?;
                    let describe = if assumed {
                        format!("{describe} (matrix assumed)")
                    } else {
                        describe
                    };
                    if !decoded_as.contains(&describe) {
                        decoded_as.push(describe);
                    }
                    if let Some(views) = &views {
                        let transform = views.views[*entry]
                            .as_ref()
                            .ok_or("a used clip has no view")?;
                        transform.apply_image(&mut px, 4);
                    }
                    let img = Image::from_data(fw as usize, fh as usize, ChannelSet::Rgba, px)
                        .ok_or("picture buffer has the wrong size")?;
                    let placement = &placements[entry];
                    resample::fit(&img, width, height, placement, [0.0, 0.0, 0.0, 1.0])
                        .map_err(|err| format!("`{}`: {err}", e.name))?
                }
            };
            let mut image = image;
            if let (Some(track), Some(font)) = (&captions, &font) {
                let at = RationalTime::exact(n, list.rate);
                if let Some(c) = track.captions.iter().find(|c| c.record.contains(at)) {
                    let placement = match c.placement {
                        cine_timeline::captions::Placement::Top => CaptionPlacement::Top,
                        cine_timeline::captions::Placement::Bottom => CaptionPlacement::Bottom,
                    };
                    burn_in(&mut image, &c.lines, placement, font)
                        .map_err(|e| format!("caption at frame {n}: {e}"))?;
                    burned += 1;
                }
            }
            match request.output {
                Output::Png8 => {
                    let bytes: Vec<u8> = image
                        .data
                        .iter()
                        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
                        .collect();
                    let png = crate::frames::png(&bytes, width as u32, height as u32)?;
                    write_file(&partial.join(frame_file(n)), &png)?;
                }
                Output::Png16 => {
                    let png = png16(&image.data, width as u32, height as u32)?;
                    write_file(&partial.join(frame_file(n)), &png)?;
                }
                Output::Exr => {
                    // Unclamped, 32-bit float: the scene values themselves.
                    write_exr(&partial.join(exr_frame_file(n)), &image.data, width, height)?;
                }
                Output::Movie => {
                    let ([y, u, v], clipped) =
                        cine_gpu::yuv::rgb_to_yuv8_cpu(&conversion, &image.data);
                    encode_clipped += clipped;
                    let writer = movie.as_mut().ok_or("the movie was not started")?;
                    sound_buf.clear();
                    let due = writer.sound_frames_due() as usize;
                    if let Some(a) = audio.as_mut() {
                        let got = a
                            .read(sound_pos, due, &mut sound_buf)
                            .map_err(|e| format!("sound at frame {n}: {e}"))?;
                        // Past the sequence's last sample the cut is silent.
                        sound_buf.resize(due * 2, 0.0);
                        let _ = got;
                        sound_pos += due as i64;
                    }
                    writer
                        .write_frame(
                            &YuvFrame {
                                width,
                                height,
                                y,
                                u,
                                v,
                            },
                            &sound_buf,
                        )
                        .map_err(|e| format!("frame {n}: {e}"))?;
                }
            }
        }
        let movie_report = match movie.take() {
            Some(w) => Some(w.finish().map_err(|e| e.to_string())?),
            None => None,
        };

        let sources: Vec<J> = used
            .iter()
            .map(|i| {
                let e = &list.entries[*i];
                let (sw, sh) = sizes[i];
                let p = &placements[i];
                json!({
                    "clip": e.clip,
                    "name": e.name,
                    "asset": e.asset,
                    "path": e.path,
                    "retimed": e.timing.is_some(),
                    "size": [sw, sh],
                    "placement": {
                        "scale": [p.scale.0, p.scale.1],
                        "covers": [p.covers.0, p.covers.1, p.covers.2, p.covers.3],
                        "keeps": [p.keeps.0, p.keeps.1, p.keeps.2, p.keeps.3],
                        "scaled": (p.covers.2, p.covers.3) != (sw, sh),
                    },
                })
            })
            .collect();
        let files = match request.output {
            Output::Movie => json!({ "movie": MOVIE_FILE }),
            Output::Exr => {
                json!({ "pattern": "frame_%06d.exr", "first": first, "count": plan.len() })
            }
            _ => json!({ "pattern": "frame_%06d.png", "first": first, "count": plan.len() }),
        };
        let precision = match request.output {
            Output::Png8 => "decoded 8-bit; processed in 32-bit float; written 8 bits per channel, clamped to [0, 1]",
            Output::Png16 => "decoded 8-bit; processed in 32-bit float; written 16 bits per channel, clamped to [0, 1]",
            Output::Movie => "decoded 8-bit; processed in 32-bit float; written 8-bit 4:2:0 BT.709 limited range (chroma averaged over 2x2), lossy H.264",
            Output::Exr => "processed and written in 32-bit float, unclamped: values above 1 and below 0 are kept as they are",
        };
        let manifest = json!({
            "kind": RENDER_KIND,
            "renderer": provenance::RENDERER,
            "request": request.to_params(),
            "sequence": { "id": list.timeline, "name": list.name,
                          "rate_num": list.rate.num(), "rate_den": list.rate.den() },
            "revision": revision,
            "frames": { "first": first, "count": plan.len() },
            "files": files,
            "width": width,
            "height": height,
            "fit": request.fit.as_str(),
            "pixel_aspect": "1:1 — sources are read as square-pixel; this build reads no pixel aspect from files",
            "precision": precision,
            "decoded_as": decoded_as,
            "encode_clipped_samples": encode_clipped,
            "look": match &views {
                None => json!({ "kind": "as_decoded" }),
                Some(v) => json!({
                    "kind": "project_view",
                    "display": v.display,
                    "view": v.view,
                    "transforms": v.describe,
                    "worst_lattice_error": v.worst_error,
                }),
            },
            "movie": movie_report.as_ref().map(|r| json!({
                "codec": "h264 constrained baseline",
                "quality_qp": request.quality,
                "colour_tags": colour.map(|c| json!({ "primaries": c.primaries, "transfer": c.transfer, "matrix": 1, "range": "limited" })),
                "keyframes": r.keyframes,
                "bytes": r.file_bytes,
            })),
            "sound": movie_report.as_ref().filter(|_| sound).map(|r| json!({
                "codec": "pcm_s24le",
                "sample_rate": SOUND_RATE,
                "channels": 2,
                "frames": r.sound_frames,
                "clipped_samples": r.sound_clipped,
                "mixer": "sequence_audio: varispeed or pitch-kept per clip retime; mono placed in both channels; muted tracks excluded",
            })),
            "captions": captions.as_ref().map(|_| json!({
                "language": request.captions,
                "frames_burned": burned,
                "font": font.as_ref().map(|f| f.path.display().to_string()),
            })),
            "sources": sources,
            "dependencies": dependencies,
            "elapsed_ms": began.elapsed().as_millis() as u64,
        });
        let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
        write_file(&partial.join("manifest.json"), text.as_bytes())?;
        Ok(Some(manifest))
    })();

    match result {
        Ok(Some(manifest)) => {
            if out.exists() {
                discard(&partial);
                return Err(format!(
                    "{} already exists; a render never replaces another",
                    out.display()
                ));
            }
            std::fs::rename(&partial, out).map_err(|e| e.to_string())?;
            Ok(json!({
                "path": out,
                "output": request.output.as_str(),
                "frames": manifest["frames"]["count"],
                "width": width,
                "height": height,
                "captions_burned": manifest["captions"]["frames_burned"],
                "sound": manifest["sound"],
                "look": manifest["look"],
                "elapsed_ms": began.elapsed().as_millis() as u64,
            }))
        }
        Ok(None) => {
            discard(&partial);
            Ok(json!({ "cancelled": true }))
        }
        Err(e) => {
            discard(&partial);
            Err(e)
        }
    }
}

/// Remove a render's partial directory — temporary data — and then its
/// sequence's render directory and `derived/render` if that left them empty.
/// `remove_dir` removes only empty directories, so no other render can go.
fn discard(partial: &Path) {
    let _ = std::fs::remove_dir_all(partial);
    let mut parent = partial.parent();
    for _ in 0..2 {
        match parent {
            Some(p) if std::fs::remove_dir(p).is_ok() => parent = p.parent(),
            _ => break,
        }
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// The EXR file name of record frame `n`.
pub fn exr_frame_file(n: i64) -> String {
    format!("frame_{n:06}.exr")
}

/// One frame as 32-bit float RGBA EXR, unclamped.
///
/// Nothing is scaled, clipped or re-encoded: the samples written are the
/// samples computed, which is the point of delivering EXR at all.
pub fn write_exr(path: &Path, rgba: &[f32], width: usize, height: usize) -> Result<(), String> {
    exr::prelude::write_rgba_file(path, width, height, |x, y| {
        let i = (y * width + x) * 4;
        (rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3])
    })
    .map_err(|e| format!("{}: {e}", path.display()))
}

/// A 16-bit-per-channel RGBA PNG of float samples, clamped to [0, 1].
pub fn png16(rgba: &[f32], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let samples: Vec<u16> = rgba
        .iter()
        .map(|v| (f64::from(v.clamp(0.0, 1.0)) * 65535.0).round() as u16)
        .collect();
    let img = image::ImageBuffer::<image::Rgba<u16>, Vec<u16>>::from_raw(w, h, samples)
        .ok_or("picture buffer has the wrong size")?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// The primaries and transfer a movie through `display` is declared with.
///
/// Only displays this writer can label truthfully: BT.709 primaries with a
/// BT.1886/BT.709 or sRGB transfer. Anything else — P3, Rec.2100, a display
/// this cannot identify — is refused rather than labelled Rec.709.
pub fn display_tags(display: &str) -> Result<ColourTags, String> {
    let d = display.to_ascii_lowercase();
    if d.contains("p3")
        || d.contains("2100")
        || d.contains("2020")
        || d.contains("pq")
        || d.contains("hlg")
    {
        return Err(format!(
            "a movie through `{display}` would be declared Rec.709 and shown wrongly; this writer declares only Rec.709-primaries displays — render PNG frames for it"
        ));
    }
    if d.contains("srgb") {
        return Ok(ColourTags::SRGB);
    }
    if d.contains("1886") || d.contains("709") {
        return Ok(ColourTags::BT709);
    }
    Err(format!(
        "`{display}` is not a display this writer can declare in a movie (Rec.1886/Rec.709 or sRGB); render PNG frames for it"
    ))
}

fn partial_dir(out: &Path) -> PathBuf {
    let mut name = out
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".partial");
    out.with_file_name(name)
}

/// What each entry needs baked, read from the graph.
pub(crate) struct WantedViews {
    pub(crate) config: cine_color::ocio::Config,
    pub(crate) config_key: u64,
    pub(crate) display: String,
    pub(crate) view: String,
    pub(crate) working: String,
    /// Per entry: input colorspace and grade.
    pub(crate) per_entry: Vec<Option<(String, Option<ucg_schema::Cdl>)>>,
}

struct Views {
    display: String,
    view: String,
    /// Per entry; `None` for entries this render does not show.
    views: Vec<Option<cine_color::view::ViewTransform>>,
    describe: Vec<String>,
    worst_error: f32,
}

pub(crate) fn wanted_views(
    project: &OmniProject,
    graph: &ucg_core::Graph,
    list: &crate::playlist::EditList,
    display: Option<String>,
    view: Option<String>,
) -> Result<WantedViews, String> {
    let (config, config_key) = cine_color::view::project_config(graph, project.root)
        .ok_or("the project has no colour config, so it has no view to render through")?;
    let (display, view) = match (display, view) {
        (Some(d), Some(v)) => (d, v),
        _ => match project.session.view() {
            Some(v) => (v.display, v.view),
            None => cine_color::view::default_view(&config)
                .ok_or("the project's colour config declares no display and view")?,
        },
    };
    let inputs = cine_color::view::input_colorspaces(graph);
    let working = config.role("scene_linear").unwrap_or("ACEScg").to_string();
    let per_entry = list
        .entries
        .iter()
        .map(|e| {
            let input = e
                .asset
                .and_then(|a| graph.node(a))
                .and_then(|n| n.get_str(ucg_schema::attrs::MEDIA_PATH))
                .and_then(|p| inputs.get(p))?
                .clone();
            let cdl = e
                .clip
                .and_then(|clip| cine_color::ucg::grade_of_clip(graph, clip));
            Some((input, cdl))
        })
        .collect();
    Ok(WantedViews {
        config,
        config_key,
        display,
        view,
        working,
        per_entry,
    })
}

/// The view for each used entry: a lattice for display-referred pictures, the
/// transform itself for scene-linear ones. `cine_color::view::view_for` makes
/// the choice from the input's encoding.
fn resolve_views(
    w: &WantedViews,
    list: &crate::playlist::EditList,
    used: &[usize],
) -> Result<Views, String> {
    let mut cache: HashMap<String, cine_color::view::ViewTransform> = HashMap::new();
    let mut views = Vec::with_capacity(list.entries.len());
    let mut describe = Vec::new();
    let mut worst = 0.0f32;
    for (i, (entry, wanted)) in list.entries.iter().zip(&w.per_entry).enumerate() {
        if !used.contains(&i) {
            views.push(None);
            continue;
        }
        let Some((input, cdl)) = wanted else {
            return Err(format!(
                "`{}` has no input colorspace established, so it cannot be rendered through {} / {}; establish one, or render as decoded",
                entry.name, w.display, w.view
            ));
        };
        let key = format!(
            "{input}|{}",
            cine_color::view::cdl_fingerprint(cdl.as_ref())
        );
        let transform = match cache.get(&key) {
            Some(t) => t.clone(),
            None => {
                let t = cine_color::view::view_for(
                    &w.config,
                    input,
                    &w.working,
                    cdl.as_ref(),
                    &w.display,
                    &w.view,
                )
                .map_err(|why| format!("`{}`: {why}", entry.name))?;
                cache.insert(key, t.clone());
                t
            }
        };
        worst = worst.max(transform.worst_error());
        if !describe.contains(&transform.describe().to_string()) {
            describe.push(transform.describe().to_string());
        }
        views.push(Some(transform));
    }
    Ok(Views {
        display: w.display.clone(),
        view: w.view.clone(),
        views,
        describe,
        worst_error: worst,
    })
}
