//! Rendering a sequence to pictures: the first delivery path.
//!
//! A render is the cut as the record monitor shows it, written to disk: every
//! record frame resolved through [`crate::playlist`] — the same edit list and
//! the same picture arithmetic the monitor and the native player use, retimes
//! and active angles included — decoded, optionally put through the project's
//! display and view by the one bake the window uses
//! ([`cine_color::view::bake_view`]), captions burned in by
//! [`cine_comp::ops::captions::burn_in`], and written as a PNG sequence.
//!
//! ## What it refuses
//!
//! Nothing here renders a picture it cannot stand behind. A clip that is
//! offline or undecodable, a picture the decoder does not produce, pictures of
//! different sizes in one cut, a view asked for but not bakeable for some clip
//! (no input colorspace established, or scene-linear input): each refuses the
//! render and names the clip. None becomes a black frame, a stretched frame or
//! a frame shown as decoded while the manifest claims a view. A gap in the cut
//! *is* black, because black is what the cut says there.
//!
//! ## Where it goes
//!
//! Renders are derived media (`docs/blueprint/STORAGE-POLICY.md`): retained,
//! never cleaned automatically, and reproducible from their manifest. A render
//! is written into `<dir>.partial` and renamed into place only when every
//! picture and the manifest are written, so a directory without `.partial`
//! is a complete render. A cancelled or failed render's partial directory is
//! temporary data and is removed.

use cine_comp::image::{ChannelSet, Image};
use cine_comp::ops::captions::{burn_in, CaptionFont, Placement};
use cine_pipeline::OmniProject;
use serde_json::{json, Value as J};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use ucg_core::{EntityId, RationalTime, Rounding};

/// The job kind.
pub const RENDER_KIND: &str = "render.sequence";

/// How pictures are coloured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Look {
    /// The decoder's RGB, through no view: what the file holds.
    AsDecoded,
    /// Through the project's display and view — the session's, or the config's
    /// default — with each clip's grade, exactly as the window shows it.
    ProjectView,
}

impl Look {
    pub fn parse(s: &str) -> Result<Look, String> {
        match s {
            "as_decoded" => Ok(Look::AsDecoded),
            "project_view" => Ok(Look::ProjectView),
            other => Err(format!(
                "unknown look `{other}` (known: as_decoded, project_view)"
            )),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Look::AsDecoded => "as_decoded",
            Look::ProjectView => "project_view",
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
    /// `None`.
    pub frames: Option<(i64, i64)>,
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
        let look = Look::parse(
            params
                .get("look")
                .and_then(J::as_str)
                .unwrap_or("as_decoded"),
        )?;
        let frames = match (
            params.get("start_frame").and_then(J::as_i64),
            params.get("end_frame").and_then(J::as_i64),
        ) {
            (None, None) => None,
            (Some(s), Some(e)) if s >= 0 && e > s => Some((s, e)),
            _ => return Err("`start_frame` and `end_frame` go together, with 0 ≤ start < end".into()),
        };
        Ok(RenderRequest {
            timeline,
            captions,
            look,
            frames,
        })
    }

    pub fn to_params(&self) -> J {
        json!({
            "timeline": self.timeline,
            "captions": self.captions,
            "look": self.look.as_str(),
            "start_frame": self.frames.map(|f| f.0),
            "end_frame": self.frames.map(|f| f.1),
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

/// Render `request` into `out`. `progress` is told how far along the render is
/// and returns `false` to cancel.
pub fn render_sequence(
    project: &OmniProject,
    request: &RenderRequest,
    out: &Path,
    progress: &mut dyn FnMut(f32, &str) -> bool,
) -> Result<J, String> {
    let began = Instant::now();

    // Everything that needs the graph is gathered under one short read, so a
    // long render never holds writers off.
    let (list, plan, captions, revision, view_wanted, first) = {
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
        let mut plan = Vec::with_capacity((last - first).max(0) as usize);
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
        let captions = match &request.captions {
            Some(language) => Some(
                cine_timeline::graph::export_captions(&graph, request.timeline, language)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| {
                        format!("the sequence has no `{language}` captions to burn in")
                    })?,
            ),
            None => None,
        };
        let view_wanted = match request.look {
            Look::AsDecoded => None,
            Look::ProjectView => Some(wanted_views(project, &graph, &list)?),
        };
        (
            list,
            plan,
            captions,
            project.engine.revision(),
            view_wanted,
            first,
        )
    };

    // Bake each clip's view once, outside the graph read.
    let views = match &view_wanted {
        None => None,
        Some(w) => {
            let used: Vec<usize> = plan
                .iter()
                .filter_map(|s| match s {
                    Planned::Picture { entry, .. } => Some(*entry),
                    Planned::Black => None,
                })
                .collect();
            Some(bake_views(w, &list, &used)?)
        }
    };

    // Open each file once; every picture must be the size of the first.
    let mut readers: HashMap<PathBuf, cine_media::VideoReader> = HashMap::new();
    let mut size: Option<(u32, u32, String)> = None;
    for step in &plan {
        let Planned::Picture { entry, .. } = step else {
            continue;
        };
        let e = &list.entries[*entry];
        let path = e.path.clone().ok_or("an entry has no file")?;
        if readers.contains_key(&path) {
            continue;
        }
        let reader = cine_media::VideoReader::open_path(&path)
            .map_err(|err| format!("`{}`: {err}", e.name))?;
        let video = reader
            .info()
            .streams
            .iter()
            .find_map(|s| s.video.as_ref())
            .ok_or_else(|| format!("`{}` has no pictures", e.name))?;
        match &size {
            None => size = Some((video.width, video.height, e.name.clone())),
            Some((w, h, name)) if (*w, *h) != (video.width, video.height) => {
                return Err(format!(
                    "`{}` is {}x{} but `{name}` is {w}x{h}: a render of mixed picture sizes would have to scale one, and this render does not scale",
                    e.name, video.width, video.height
                ));
            }
            _ => {}
        }
        readers.insert(path, reader);
    }
    let Some((width, height, _)) = size else {
        return Err("there is no picture in that part of the cut to render".into());
    };
    let font = match &captions {
        Some(_) => Some(CaptionFont::system().map_err(|e| e.to_string())?),
        None => None,
    };

    let partial = partial_dir(out);
    let result = (|| -> Result<Option<J>, String> {
        std::fs::create_dir_all(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
        let mut burned = 0usize;
        let mut decoded_as: Vec<String> = Vec::new();
        let count = plan.len();
        for (k, step) in plan.iter().enumerate() {
            let n = first + k as i64;
            if !progress(k as f32 / count as f32, &format!("frame {n}")) {
                return Ok(None);
            }
            let mut rgba = match step {
                Planned::Black => {
                    let mut px = vec![0.0f32; width as usize * height as usize * 4];
                    for a in px.as_chunks_mut::<4>().0 {
                        a[3] = 1.0;
                    }
                    px
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
                    let (mut px, _, _, describe, assumed) = crate::frames::rgba_f32(&frame)?;
                    let describe = if assumed {
                        format!("{describe} (matrix assumed)")
                    } else {
                        describe
                    };
                    if !decoded_as.contains(&describe) {
                        decoded_as.push(describe);
                    }
                    if let Some(views) = &views {
                        let lut = views.luts[*entry].as_ref().ok_or("a used clip was not baked")?;
                        for p in px.as_chunks_mut::<4>().0 {
                            let [r, g, b] = lut.apply([p[0], p[1], p[2]]);
                            p[0] = r;
                            p[1] = g;
                            p[2] = b;
                        }
                    }
                    px
                }
            };
            if let (Some(track), Some(font)) = (&captions, &font) {
                let at = RationalTime::exact(n, list.rate);
                if let Some(c) = track.captions.iter().find(|c| c.record.contains(at)) {
                    let mut image = Image::from_data(
                        width as usize,
                        height as usize,
                        ChannelSet::Rgba,
                        std::mem::take(&mut rgba),
                    )
                    .ok_or("picture buffer has the wrong size")?;
                    let placement = match c.placement {
                        cine_timeline::captions::Placement::Top => Placement::Top,
                        cine_timeline::captions::Placement::Bottom => Placement::Bottom,
                    };
                    burn_in(&mut image, &c.lines, placement, font)
                        .map_err(|e| format!("caption at frame {n}: {e}"))?;
                    rgba = image.data;
                    burned += 1;
                }
            }
            let bytes: Vec<u8> = rgba
                .iter()
                .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
                .collect();
            let png = crate::frames::png(&bytes, width, height)?;
            let file = partial.join(frame_file(n));
            std::fs::write(&file, png).map_err(|e| format!("{}: {e}", file.display()))?;
        }

        let sources: Vec<J> = {
            let mut seen = Vec::new();
            for step in &plan {
                if let Planned::Picture { entry, .. } = step {
                    if !seen.contains(entry) {
                        seen.push(*entry);
                    }
                }
            }
            seen.iter()
                .map(|i| {
                    let e = &list.entries[*i];
                    json!({
                        "clip": e.clip,
                        "name": e.name,
                        "asset": e.asset,
                        "path": e.path,
                        "retimed": e.timing.is_some(),
                    })
                })
                .collect()
        };
        let manifest = json!({
            "kind": RENDER_KIND,
            "request": request.to_params(),
            "sequence": { "id": list.timeline, "name": list.name,
                          "rate_num": list.rate.num(), "rate_den": list.rate.den() },
            "revision": revision,
            "frames": { "first": first, "count": plan.len(), "pattern": "frame_%06d.png" },
            "width": width,
            "height": height,
            "encoding": "8-bit RGBA PNG; values clamped to [0, 1] after the look",
            "decoded_as": decoded_as,
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
            "captions": captions.as_ref().map(|_| json!({
                "language": request.captions,
                "frames_burned": burned,
                "font": font.as_ref().map(|f| f.path.display().to_string()),
            })),
            "sources": sources,
            "elapsed_ms": began.elapsed().as_millis() as u64,
        });
        let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
        std::fs::write(partial.join("manifest.json"), text).map_err(|e| e.to_string())?;
        Ok(Some(manifest))
    })();

    match result {
        Ok(Some(manifest)) => {
            if out.exists() {
                return Err(format!(
                    "{} already exists; a render never replaces another",
                    out.display()
                ));
            }
            std::fs::rename(&partial, out).map_err(|e| e.to_string())?;
            Ok(json!({
                "path": out,
                "frames": manifest["frames"]["count"],
                "width": width,
                "height": height,
                "captions_burned": manifest["captions"]["frames_burned"],
                "look": manifest["look"],
                "elapsed_ms": began.elapsed().as_millis() as u64,
            }))
        }
        Ok(None) => {
            let _ = std::fs::remove_dir_all(&partial);
            Ok(json!({ "cancelled": true }))
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&partial);
            Err(e)
        }
    }
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
struct WantedViews {
    config: cine_color::ocio::Config,
    display: String,
    view: String,
    working: String,
    /// Per entry: input colorspace and grade.
    per_entry: Vec<Option<(String, Option<ucg_schema::Cdl>)>>,
}

struct Views {
    display: String,
    view: String,
    /// Per entry; `None` for entries this render does not show.
    luts: Vec<Option<Arc<cine_color::lut::Lut3D>>>,
    describe: Vec<String>,
    worst_error: f32,
}

fn wanted_views(
    project: &OmniProject,
    graph: &ucg_core::Graph,
    list: &crate::playlist::EditList,
) -> Result<WantedViews, String> {
    let (config, _) = cine_color::view::project_config(graph, project.root)
        .ok_or("the project has no colour config, so it has no view to render through")?;
    let (display, view) = match project.session.view() {
        Some(v) => (v.display, v.view),
        None => cine_color::view::default_view(&config)
            .ok_or("the project's colour config declares no display and view")?,
    };
    let inputs = cine_color::view::input_colorspaces(graph);
    let working = config.role("scene_linear").unwrap_or("ACEScg").to_string();
    let per_entry = list
        .entries
        .iter()
        .map(|e| {
            let input = e
                .path
                .as_ref()
                .and_then(|p| inputs.get(&p.display().to_string()))?
                .clone();
            let cdl = e
                .clip
                .and_then(|clip| cine_color::ucg::grade_of_clip(graph, clip));
            Some((input, cdl))
        })
        .collect();
    Ok(WantedViews {
        config,
        display,
        view,
        working,
        per_entry,
    })
}

fn bake_views(
    w: &WantedViews,
    list: &crate::playlist::EditList,
    used: &[usize],
) -> Result<Views, String> {
    let mut cache: HashMap<String, cine_color::view::BakedView> = HashMap::new();
    let mut luts = Vec::with_capacity(list.entries.len());
    let mut describe = Vec::new();
    let mut worst = 0.0f32;
    for (i, (entry, wanted)) in list.entries.iter().zip(&w.per_entry).enumerate() {
        if !used.contains(&i) {
            luts.push(None);
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
        let baked = match cache.get(&key) {
            Some(b) => b.clone(),
            None => {
                let b = cine_color::view::bake_view(
                    &w.config,
                    input,
                    &w.working,
                    cdl.as_ref(),
                    &w.display,
                    &w.view,
                )
                .ok_or_else(|| {
                    format!(
                        "`{}` ({input}) cannot be baked into {} / {}: the transform does not compile, or the input is scene-linear",
                        entry.name, w.display, w.view
                    )
                })?;
                cache.insert(key, b.clone());
                b
            }
        };
        worst = worst.max(baked.worst_error);
        if !describe.contains(&baked.describe) {
            describe.push(baked.describe.clone());
        }
        luts.push(Some(baked.lut));
    }
    Ok(Views {
        display: w.display.clone(),
        view: w.view.clone(),
        luts,
        describe,
        worst_error: worst,
    })
}
