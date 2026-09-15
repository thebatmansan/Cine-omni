//! Client sessions against one authoritative project.
//!
//! ## The defect this replaces
//!
//! The previous architecture spawned one `cine serve` process per browser
//! connection, each opening the project independently. Two windows meant two
//! engines, two op logs and two divergent notions of the truth, with no
//! mechanism that would ever reconcile them. Whichever saved last won, and the
//! other window's work was gone.
//!
//! Here there is exactly one [`OmniProject`] and one `Engine`. Clients are
//! views onto it. A commit made by any client is broadcast to every client,
//! including the one that made it, because the engine's revision is the shared
//! truth and a client that tracked its own optimistic state would drift.

use cine_pipeline::OmniProject;
use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Identifies one connected client for the lifetime of the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClientId(pub u64);

impl std::fmt::Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "client-{}", self.0)
    }
}

/// A connected client's outbound queue.
struct Client {
    id: ClientId,
    outbound: Sender<String>,
    /// What the client called itself, for diagnostics.
    agent: String,
}

/// The shared application runtime.
pub struct Runtime {
    /// One open decoder, shared by every client asking for a picture.
    pub frames: crate::frames::FrameService,
    pub project: Arc<OmniProject>,
    /// Where the project lives on disk, for reporting and for the instance lock.
    pub root_dir: std::path::PathBuf,
    clients: Mutex<Vec<Client>>,
    next_id: AtomicU64,
    started: std::time::Instant,
}

impl Runtime {
    pub fn new(project: OmniProject, root_dir: std::path::PathBuf) -> Arc<Runtime> {
        let project = Arc::new(project);
        // Background work needs the project and its media, so it starts here
        // rather than in the bare project. Failing to start it is reported, and
        // the `jobs` capability says so; the project still opens.
        match crate::jobs::start(&project, &root_dir) {
            Ok(jobs) => {
                let _ = project.jobs.set(jobs);
            }
            Err(e) => eprintln!("background work unavailable: {e}"),
        }
        Arc::new(Runtime {
            project,
            root_dir,
            clients: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
            started: std::time::Instant::now(),
            frames: crate::frames::FrameService::new(),
        })
    }

    /// Register a client and hand back its inbound queue.
    ///
    /// The queue is bounded: a client that stops reading must not be able to
    /// make the runtime allocate without limit. When it overflows the client is
    /// dropped, which is the correct outcome — a client that cannot keep up with
    /// change notifications has already lost coherence with the project.
    pub fn connect(&self, agent: &str) -> (ClientId, Receiver<String>) {
        let id = ClientId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = crossbeam_channel::bounded(1024);
        self.clients.lock().push(Client {
            id,
            outbound: tx,
            agent: agent.to_string(),
        });
        (id, rx)
    }

    pub fn disconnect(&self, id: ClientId) {
        self.clients.lock().retain(|c| c.id != id);
    }

    pub fn client_count(&self) -> usize {
        self.clients.lock().len()
    }

    pub fn client_agents(&self) -> Vec<(ClientId, String)> {
        self.clients
            .lock()
            .iter()
            .map(|c| (c.id, c.agent.clone()))
            .collect()
    }

    pub fn uptime(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    /// Send one message to every connected client.
    ///
    /// Returns how many clients were dropped for being unable to keep up.
    pub fn broadcast(&self, message: &str) -> usize {
        let mut clients = self.clients.lock();
        let before = clients.len();
        clients.retain(|c| c.outbound.try_send(message.to_string()).is_ok());
        before - clients.len()
    }

    /// Send to one client, if it is still connected.
    pub fn send_to(&self, id: ClientId, message: &str) -> bool {
        self.clients
            .lock()
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.outbound.try_send(message.to_string()).is_ok())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cine_pipeline::ProjectSetup;

    fn runtime() -> Arc<Runtime> {
        let project = OmniProject::new(ProjectSetup::default()).expect("project");
        Runtime::new(project, std::path::PathBuf::from("/tmp/none"))
    }

    #[test]
    fn every_client_sees_a_broadcast_including_the_one_that_caused_it() {
        let rt = runtime();
        let (_a, rx_a) = rt.connect("window-a");
        let (_b, rx_b) = rt.connect("window-b");
        assert_eq!(rt.client_count(), 2);

        assert_eq!(
            rt.broadcast("{\"revision\":7}"),
            0,
            "nothing should be dropped"
        );
        assert_eq!(rx_a.try_recv().unwrap(), "{\"revision\":7}");
        assert_eq!(
            rx_b.try_recv().unwrap(),
            "{\"revision\":7}",
            "a second window must see the first window's commit, or the two diverge"
        );
    }

    #[test]
    fn clients_share_one_project_rather_than_one_each() {
        let rt = runtime();
        let (_a, _rx_a) = rt.connect("a");
        let (_b, _rx_b) = rt.connect("b");
        // One engine, one revision counter. Two `cine serve` processes would
        // each have started from zero and diverged from here.
        let before = rt.project.engine.revision();
        rt.project
            .engine
            .transact("touch", |tx| {
                tx.set(rt.project.root, "notes", "edited by one window")
            })
            .unwrap();
        assert!(rt.project.engine.revision() > before);
        assert_eq!(
            rt.client_count(),
            2,
            "both clients still observe the same engine"
        );
    }

    #[test]
    fn a_disconnected_client_stops_receiving() {
        let rt = runtime();
        let (a, rx_a) = rt.connect("a");
        rt.disconnect(a);
        assert_eq!(rt.client_count(), 0);
        rt.broadcast("after");
        assert!(rx_a.try_recv().is_err());
    }

    /// A client that never drains its queue must not grow the runtime's memory
    /// without limit; it is dropped instead.
    #[test]
    fn a_client_that_stops_reading_is_dropped_rather_than_buffered_forever() {
        let rt = runtime();
        let (_id, _rx) = rt.connect("stalled");
        let mut dropped = 0;
        for i in 0..5_000 {
            dropped += rt.broadcast(&format!("{{\"n\":{i}}}"));
            if dropped > 0 {
                break;
            }
        }
        assert_eq!(
            dropped, 1,
            "the stalled client should have been dropped exactly once"
        );
        assert_eq!(rt.client_count(), 0);
    }

    #[test]
    fn send_to_reaches_only_the_named_client() {
        let rt = runtime();
        let (a, rx_a) = rt.connect("a");
        let (_b, rx_b) = rt.connect("b");
        assert!(rt.send_to(a, "just for a"));
        assert_eq!(rx_a.try_recv().unwrap(), "just for a");
        assert!(rx_b.try_recv().is_err());
        // An unknown client is reported, not silently ignored.
        assert!(!rt.send_to(ClientId(999), "nobody"));
    }
}
