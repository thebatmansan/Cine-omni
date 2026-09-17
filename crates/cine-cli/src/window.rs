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
use ucg_schema::kinds;

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
            format!(
                "{}|{}|{}",
                want.input,
                want.working,
                cine_color::view::cdl_fingerprint(want.cdl)
            ),
            want.display.to_string(),
            want.view.to_string(),
        );
        if let Some(hit) = self.lattices.lock().get(&key) {
            return hit.clone();
        }
        let baked = cine_color::view::bake_view(
            config,
            want.input,
            want.working,
            want.cdl,
            want.display,
            want.view,
        )
        .map(|b| (b.lut, b.worst_error, b.describe));
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
                timing: e.timing.clone(),
                name: e.name.clone(),
            })
            .collect();
        let sound = list
            .sound
            .iter()
            .map(|e| SoundEntry {
                record: e.record,
                path: e.unplayable().is_none().then(|| e.path.clone()).flatten(),
                offset: e.offset,
                name: e.name.clone(),
                // Read at its speed by the mixer: varispeed, or stretched with
                // its pitch kept, as the clip's retime says.
                // A sound entry is the whole clip, so its map is anchored at the
                // entry's own start, where `offset` is measured.
                map: e.timing.as_ref().map(|t| t.map.clone()),
                keep_pitch: e.keep_pitch,
            })
            .collect();
        Some(SequencePlan {
            sound,
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
        let (config, config_key) = cine_color::view::project_config(&graph, root)?;
        let (display, view) = match self.runtime.project.session.view() {
            Some(v) => (v.display, v.view),
            None => cine_color::view::default_view(&config)?,
        };
        let id = current_timeline(&graph, self.runtime.project.session.focus())?;
        let list = cine_runtime::playlist::edit_list(&graph, id).ok()?;
        let inputs = cine_color::view::input_colorspaces(&graph);
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
