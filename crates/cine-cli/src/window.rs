//! The native window as a surface of the running project.
//!
//! Implements `cine_shell::link::ProjectLink` over the runtime's own engine, in
//! the same process. The window reads the flattened edit list the runtime
//! builds for the browser's record monitor, subscribes to the same commits the
//! WebSocket broadcaster forwards, and sends its few intents through the same
//! graph operations the protocol uses. There is no second engine, no second
//! op log and no second composition.

use cine_color::lut::Lut3D;
use cine_color::ocio::Config;
use cine_media::{EditEntry, SoundEntry};
use cine_runtime::Runtime;
use cine_shell::link::{ProjectLink, SequencePlan, Viewing};
use cine_timeline::session::TrackState;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use ucg_core::{EntityId, Graph, UcgError};
use ucg_schema::{attrs, kinds, ColorMeta, Component};

/// Edge length of a baked view lattice.
///
/// 33 is the size grading systems have used for decades, and the error it
/// leaves is measured rather than assumed: every `Viewing` carries the worst
/// difference between the lattice and the transform it came from.
const LATTICE_SIZE: usize = 33;

/// The domain a lattice covers, in code values. Limited-range video reaches
/// below 0 and above 1 (sub-black and super-white), and a viewer that clipped
/// them at the lattice edge would hide exactly what a colourist looks for.
const LATTICE_DOMAIN: (f32, f32) = (-0.125, 1.125);

type Waker = Arc<dyn Fn() + Send + Sync>;

type Lattice = Option<(Arc<Lut3D>, f32, String)>;

/// What a lattice is wanted for: one clip's colorspace and grade, through one
/// display and view.
struct Wanted<'a> {
    input: &'a str,
    working: &'a str,
    cdl: Option<&'a ucg_schema::Cdl>,
    display: &'a str,
    view: &'a str,
}

pub struct RuntimeLink {
    runtime: Arc<Runtime>,
    address: Option<String>,
    quit: Arc<AtomicBool>,
    wakers: Arc<Mutex<Vec<Waker>>>,
    /// Baked view transforms, by config, input colorspace, display and view.
    lattices: Mutex<HashMap<(u64, String, String, String), Lattice>>,
}

/// Lets a signal handler ask the window to close.
#[derive(Clone)]
pub struct QuitHandle {
    quit: Arc<AtomicBool>,
    wakers: Arc<Mutex<Vec<Waker>>>,
}

impl QuitHandle {
    pub fn request(&self) {
        self.quit.store(true, Ordering::Relaxed);
        for wake in self.wakers.lock().iter() {
            wake();
        }
    }
}

impl RuntimeLink {
    pub fn new(runtime: Arc<Runtime>, address: Option<String>) -> RuntimeLink {
        RuntimeLink {
            runtime,
            address,
            quit: Arc::new(AtomicBool::new(false)),
            wakers: Arc::new(Mutex::new(Vec::new())),
            lattices: Mutex::new(HashMap::new()),
        }
    }

    /// The transform for one input colorspace through one display and view,
    /// baked and measured. Cached: playback asks for the same few, per picture.
    fn lattice(&self, config: &Config, config_key: u64, want: &Wanted<'_>) -> Lattice {
        let key = (
            config_key,
            format!("{}|{}|{}", want.input, want.working, fingerprint(want.cdl)),
            want.display.to_string(),
            want.view.to_string(),
        );
        if let Some(hit) = self.lattices.lock().get(&key) {
            return hit.clone();
        }
        let baked = bake(config, want);
        self.lattices.lock().insert(key, baked.clone());
        baked
    }

    pub fn quit_handle(&self) -> QuitHandle {
        QuitHandle {
            quit: self.quit.clone(),
            wakers: self.wakers.clone(),
        }
    }
}

/// The sequence the window plays: the one open in the application, or — before
/// any surface has opened one — the most recently created.
fn current_timeline(graph: &Graph, focus: Option<EntityId>) -> Option<EntityId> {
    focus
        .filter(|f| {
            graph
                .node(*f)
                .is_some_and(|n| n.kind.as_str() == kinds::NLE_TIMELINE)
        })
        .or_else(|| {
            graph
                .of_kind(kinds::NLE_TIMELINE)
                .iter()
                .max_by_key(|n| (n.created_rev, n.id))
                .map(|n| n.id)
        })
}

fn track_ids(graph: &Graph, timeline: EntityId) -> Vec<EntityId> {
    let mut out = Vec::new();
    for stack in graph.children(timeline) {
        for child in graph.children(stack) {
            if graph
                .node(child)
                .is_some_and(|n| n.kind.as_str() == kinds::NLE_TRACK)
            {
                out.push(child);
            }
        }
    }
    out
}

impl ProjectLink for RuntimeLink {
    fn name(&self) -> String {
        self.runtime
            .root_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".into())
    }

    fn address(&self) -> Option<String> {
        self.address.clone()
    }

    fn revision(&self) -> u64 {
        self.runtime.project.engine.revision()
    }

    fn sequence(&self) -> Option<SequencePlan> {
        let graph = self.runtime.project.engine.read();
        let id = current_timeline(&graph, self.runtime.project.session.focus())?;
        let list = cine_runtime::playlist::edit_list(&graph, id).ok()?;
        let timeline = cine_timeline::graph::export_timeline(&graph, id).ok()?;
        let entries = list
            .entries
            .iter()
            .map(|e| EditEntry {
                record: e.record,
                path: e.path.clone(),
                offset: e.offset,
                unplayable: e.unplayable().map(str::to_string),
                speed: e.speed,
                name: e.name.clone(),
            })
            .collect();
        let sound = list
            .sound
            .iter()
            .map(|e| SoundEntry {
                record: e.record,
                // Retimed sound would have to be resampled to keep its pitch,
                // which the mixer does not do, so it is not handed to it: the
                // sequence plan says why below rather than playing it wrong.
                path: (e.unplayable().is_none() && e.speed.is_none())
                    .then(|| e.path.clone())
                    .flatten(),
                offset: e.offset,
                name: e.name.clone(),
            })
            .collect();
        let sound_not_played = list
            .sound
            .iter()
            .filter(|e| e.speed.is_some())
            .map(|e| {
                format!(
                    "{}: retimed sound would have to be resampled to keep its pitch",
                    e.name
                )
            })
            .collect();
        Some(SequencePlan {
            sound,
            sound_not_played,
            id,
            name: list.name.clone(),
            rate: list.rate,
            end: list.end,
            entries,
            transitions: list.transitions,
            tracks: track_ids(&graph, id),
            timeline,
        })
    }

    fn viewing(&self) -> Option<Viewing> {
        let graph = self.runtime.project.engine.read();
        let root = self.runtime.project.root;
        let config_entity = cine_color::ucg::project_config(&graph, root)?;
        let yaml = graph.node(config_entity)?.get_str("yaml")?.to_string();
        let config = Config::parse(&yaml).ok()?;
        let config_key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            yaml.hash(&mut h);
            h.finish()
        };
        let (display, view) = match self.runtime.project.session.view() {
            Some(v) => (v.display, v.view),
            None => default_view(&config)?,
        };
        let id = current_timeline(&graph, self.runtime.project.session.focus())?;
        let list = cine_runtime::playlist::edit_list(&graph, id).ok()?;
        let inputs = input_colorspaces(&graph);
        let working = config.role("scene_linear").unwrap_or("ACEScg").to_string();
        let mut luts = Vec::with_capacity(list.entries.len());
        let mut describe: Vec<String> = Vec::new();
        let mut worst = 0.0f32;
        for entry in &list.entries {
            let input = entry
                .path
                .as_ref()
                .and_then(|p| inputs.get(&p.display().to_string()));
            // The clip's own grade, when it has one.
            let cdl = entry
                .clip
                .and_then(|clip| cine_color::ucg::grade_of_clip(&graph, clip));
            let baked = input.and_then(|input| {
                self.lattice(
                    &config,
                    config_key,
                    &Wanted {
                        input,
                        working: &working,
                        cdl: cdl.as_ref(),
                        display: &display,
                        view: &view,
                    },
                )
            });
            match baked {
                Some((lut, error, what)) => {
                    worst = worst.max(error);
                    if !describe.contains(&what) {
                        describe.push(what);
                    }
                    luts.push(Some(lut));
                }
                None => luts.push(None),
            }
        }
        Some(Viewing {
            display,
            view,
            luts,
            describe,
            worst_error: worst,
        })
    }

    fn subscribe(&self, wake: Box<dyn Fn() + Send + Sync>) {
        let wake: Waker = Arc::from(wake);
        self.wakers.lock().push(wake.clone());
        let on_session = wake.clone();
        self.runtime
            .project
            .session
            .subscribe(move |_| on_session());
        self.runtime.project.engine.subscribe(move |_| wake());
    }

    fn should_quit(&self) -> bool {
        self.quit.load(Ordering::Relaxed)
    }

    fn set_track_state(&self, track: EntityId, state: TrackState) -> Result<(), String> {
        self.runtime
            .project
            .engine
            .transact("track state (window)", |tx| {
                cine_timeline::graph::set_track_state(tx, track, state)
                    .map_err(|e| UcgError::Aborted(e.to_string()))
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// The display and view a project is looked through until someone chooses:
/// the config's first active display, and its first active view.
fn default_view(config: &Config) -> Option<(String, String)> {
    let display = config
        .active_displays
        .iter()
        .find(|d| config.displays.contains_key(*d))
        .cloned()
        .or_else(|| config.displays.keys().next().cloned())?;
    let views = config.views(&display).ok()?;
    let view = config
        .active_views
        .iter()
        .find(|v| views.iter().any(|x| x.name.eq_ignore_ascii_case(v)))
        .cloned()
        .or_else(|| views.first().map(|v| v.name.clone()))?;
    Some((display, view))
}

/// Media path → the input colorspace established for it.
fn input_colorspaces(graph: &Graph) -> HashMap<String, String> {
    graph
        .of_kind(kinds::ASSET_MEDIA)
        .iter()
        .filter_map(|n| {
            let path = n.get_str(attrs::MEDIA_PATH)?.to_string();
            let meta = ColorMeta::read_from(graph, n.id)?;
            Some((path, meta.input_colorspace))
        })
        .collect()
}

/// Bake `input → display / view` to a lattice, and measure what baking cost.
///
/// `None` when the config cannot compile the transform, and — deliberately —
/// when the input colorspace is scene-linear: a lattice over code values would
/// be wrong for unbounded scene light, and showing the picture untransformed
/// while claiming a view is worse than showing it plainly.
fn bake(config: &Config, want: &Wanted<'_>) -> Lattice {
    let (input, working, cdl, display, view) =
        (want.input, want.working, want.cdl, want.display, want.view);
    let encoding = config
        .colorspace(input)
        .map(|c| c.encoding.to_lowercase())
        .unwrap_or_default();
    if !matches!(
        encoding.as_str(),
        "sdr-video" | "hdr-video" | "edr-video" | "log"
    ) {
        return None;
    }
    let processor =
        cine_color::ucg::viewing_processor(config, input, working, cdl, display, view).ok()?;
    let mut lut = Lut3D::bake_domain(
        &processor,
        LATTICE_SIZE,
        [LATTICE_DOMAIN.0; 3],
        [LATTICE_DOMAIN.1; 3],
    );
    // Trilinear, because that is what the shader does; measuring tetrahedral
    // error would be measuring an interpolation nobody performs.
    lut.interpolation = cine_color::lut::Interpolation::Trilinear;
    let error = lut.worst_error(&processor, 4096);
    let what = match cdl {
        Some(_) => format!("{input} → grade → {display} / {view}"),
        None => format!("{input} → {display} / {view}"),
    };
    Some((Arc::new(lut), error, what))
}

/// A short, stable name for a grade, so the lattice cache can tell two grades
/// apart without holding the grade itself.
fn fingerprint(cdl: Option<&ucg_schema::Cdl>) -> String {
    let Some(cdl) = cdl else {
        return "ungraded".into();
    };
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for v in cdl
        .slope
        .iter()
        .chain(&cdl.offset)
        .chain(&cdl.power)
        .chain(std::iter::once(&cdl.saturation))
    {
        v.to_bits().hash(&mut h);
    }
    format!("{:016x}", h.finish())
}
