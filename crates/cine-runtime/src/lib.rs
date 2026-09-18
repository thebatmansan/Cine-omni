//! # cine-runtime — the CINE-OMNI application runtime
//!
//! One project, one engine, many clients.
//!
//! ## What this crate replaced
//!
//! The previous architecture ran a Node bridge that spawned one `cine serve`
//! process per browser connection. Every window therefore opened the project
//! independently: two windows meant two engines, two op logs, and two
//! divergent notions of the truth with no mechanism that would ever reconcile
//! them. The last save won and the other window's work was gone.
//!
//! Here the runtime owns the project. Clients are views onto it, commits are
//! broadcast to all of them, and the engine's revision is the shared truth.
//! Two windows cannot diverge because there is only one thing to diverge from.
//!
//! ```text
//!                     cine start
//!                         │
//!                 ┌───────┴────────┐
//!                 │    Runtime     │   one OmniProject, one Engine
//!                 └───────┬────────┘
//!             ┌───────────┼───────────┐
//!             │           │           │
//!          window      window        CLI
//!             └───────────┼───────────┘
//!                         │
//!                   same UCG state
//! ```
//!
//! ## Shape
//!
//! * [`session::Runtime`] — the project and the client registry.
//! * [`rpc`] — JSON-RPC dispatch, shared by the stdio and WebSocket transports
//!   so a method cannot behave differently depending on how it was called.
//! * [`ws`] — RFC 6455 framing.
//! * [`http`] — static UI assets, `/health`, and the upgrade.
//! * [`lock`] — one runtime per project, so a second terminal attaches rather
//!   than corrupting the op log.
//!
//! Synchronous and thread-per-client throughout, matching the rest of the
//! workspace. This serves a handful of local windows, not the public internet.

pub mod derived;
pub mod frames;
pub mod http;
pub mod impact;
pub mod ingest;
pub mod jobs;
pub mod lock;
pub mod playlist;
pub mod projection;
pub mod provenance;
pub mod proxy;
pub mod relink;
pub mod render;
pub mod rpc;
pub mod server;
pub mod session;
pub mod waveform;
pub mod ws;

pub use lock::{InstanceGuard, InstanceInfo, LockError};
pub use server::{ServeConfig, Server};
pub use session::{ClientId, Runtime};
