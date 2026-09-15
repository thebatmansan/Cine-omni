//! The native window as a surface of the running project.
//!
//! Implements `cine_shell::link::ProjectLink` over the runtime's own engine, in
//! the same process. The window reads the flattened edit list the runtime
//! builds for the browser's record monitor, subscribes to the same commits the
//! WebSocket broadcaster forwards, and sends its few intents through the same
//! graph operations the protocol uses. There is no second engine, no second
//! op log and no second composition.

use cine_media::EditEntry;
use cine_runtime::Runtime;
use cine_shell::link::{ProjectLink, SequencePlan};
use cine_timeline::session::TrackState;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use ucg_core::{EntityId, Graph, UcgError};
use ucg_schema::kinds;

type Waker = Arc<dyn Fn() + Send + Sync>;

pub struct RuntimeLink {
    runtime: Arc<Runtime>,
    address: Option<String>,
    quit: Arc<AtomicBool>,
    wakers: Arc<Mutex<Vec<Waker>>>,
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
        }
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
                name: e.name.clone(),
            })
            .collect();
        Some(SequencePlan {
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
