//! What the people working on an open project are looking at, together.
//!
//! Not project state: nothing here is in the graph, in the op log, or on disk,
//! and closing the project forgets it. It is the state that makes several
//! surfaces of one application behave as one application — when the cutter
//! opens a sequence in the browser, the native window plays that sequence.
//!
//! It lives beside the engine, in the open project, so that every transport
//! and every surface reaches the same value rather than each keeping its own.

use parking_lot::{Mutex, RwLock};
use ucg_core::EntityId;

/// A change to the shared session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// The sequence open in the application changed.
    Focus(Option<EntityId>),
    /// The display and view everything is being looked at through changed.
    /// `None` means the project config's own default.
    View(Option<ViewSelection>),
}

/// A display and one of its views, by name, as the colour config names them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewSelection {
    pub display: String,
    pub view: String,
}

type Listener = Box<dyn Fn(&SessionEvent) + Send + Sync>;

#[derive(Default)]
pub struct ProjectSession {
    focus: RwLock<Option<EntityId>>,
    view: RwLock<Option<ViewSelection>>,
    listeners: Mutex<Vec<Listener>>,
}

impl ProjectSession {
    /// The sequence open in the application, if one has been opened.
    pub fn focus(&self) -> Option<EntityId> {
        *self.focus.read()
    }

    /// Open a sequence everywhere. Returns whether anything changed; listeners
    /// hear only about real changes.
    pub fn set_focus(&self, timeline: Option<EntityId>) -> bool {
        {
            let mut focus = self.focus.write();
            if *focus == timeline {
                return false;
            }
            *focus = timeline;
        }
        let event = SessionEvent::Focus(timeline);
        for l in self.listeners.lock().iter() {
            l(&event);
        }
        true
    }

    /// What everything is being looked at through, if anyone has chosen.
    pub fn view(&self) -> Option<ViewSelection> {
        self.view.read().clone()
    }

    /// Look through this display and view everywhere. `None` returns to the
    /// config's default. Returns whether anything changed.
    pub fn set_view(&self, view: Option<ViewSelection>) -> bool {
        {
            let mut current = self.view.write();
            if *current == view {
                return false;
            }
            *current = view.clone();
        }
        let event = SessionEvent::View(view);
        for l in self.listeners.lock().iter() {
            l(&event);
        }
        true
    }

    /// Hear every change. Listeners must be cheap and must not call back into
    /// the session.
    pub fn subscribe(&self, f: impl Fn(&SessionEvent) + Send + Sync + 'static) {
        self.listeners.lock().push(Box::new(f));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn listeners_hear_real_changes_only() {
        let s = ProjectSession::default();
        let heard = Arc::new(AtomicUsize::new(0));
        let h = heard.clone();
        s.subscribe(move |_| {
            h.fetch_add(1, Ordering::Relaxed);
        });
        let t = EntityId::from_key("timeline");
        assert!(s.set_focus(Some(t)));
        assert!(
            !s.set_focus(Some(t)),
            "setting the same focus is not a change"
        );
        assert_eq!(heard.load(Ordering::Relaxed), 1);
        assert_eq!(s.focus(), Some(t));
    }
}
