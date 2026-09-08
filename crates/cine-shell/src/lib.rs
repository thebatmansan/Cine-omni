//! # cine-shell — the native application shell
//!
//! The shell decision, made in `docs/blueprint/research/11-shell-and-gpu-runtime.md`,
//! is a native Rust window: `winit` + `wgpu`, sharing **one `wgpu::Device` and
//! `wgpu::Queue`** with `cine-gpu`'s compute kernels. That sharing is the whole
//! point. A decoded frame has to reach the screen without leaving the GPU, and
//! a webview architecture provably cannot do that — which is why the browser UI
//! is being repositioned as the remote review surface rather than the editing
//! one.
//!
//! ## What is here, and what is deliberately not
//!
//! This is the presentation foundation: surface configuration, the swapchain
//! loop, and [`viewer::FrameUploader`], which turns a decoded YUV frame into an
//! RGB texture using the colour path that `cine-gpu` verifies against a CPU
//! reference on real hardware.
//!
//! There is **no playback clock, no transport and no docking UI yet**, and none
//! is simulated. A control that has no backend does not appear; see
//! [`Availability`].
//!
//! ## The rule this crate exists to keep
//!
//! `ucg-core` must never gain a dependency on any UI crate. The shell is a
//! replaceable leaf: if the toolkit choice proves wrong, this crate is replaced
//! and nothing beneath it changes. Nothing below `cine-shell` may `use` a
//! windowing or GUI crate.

pub mod gpu;
pub mod present;
pub mod viewer;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ShellError {
    #[error("no GPU adapter compatible with the window surface")]
    NoAdapter,
    #[error("gpu: {0}")]
    Gpu(String),
    #[error("window: {0}")]
    Window(String),
    #[error("surface: {0}")]
    Surface(String),
    #[error("media: {0}")]
    Media(#[from] cine_media::Error),
}

pub type Result<T> = std::result::Result<T, ShellError>;

/// Whether a piece of the interface is actually backed by something.
///
/// The project rule is that a visible control either works or says plainly that
/// it does not. Rendering a transport bar that cannot transport, or a scope that
/// scopes nothing, teaches the user something false about the system. This type
/// makes the distinction explicit so a panel cannot accidentally pretend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// Backed end to end.
    Working,
    /// Deliberately absent, with the reason shown to the user.
    Unavailable(&'static str),
}

impl Availability {
    pub fn is_working(&self) -> bool {
        matches!(self, Availability::Working)
    }

    /// What to show the user in place of a control that does not work.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Availability::Working => None,
            Availability::Unavailable(why) => Some(why),
        }
    }
}

/// The capabilities the shell can honestly offer today.
///
/// Written as data rather than scattered through the UI so the honest answer is
/// in one place and can be tested.
pub struct ShellCapabilities;

impl ShellCapabilities {
    pub const VIDEO_DISPLAY: Availability = Availability::Working;
    pub const FRAME_STEP: Availability = Availability::Working;
    pub const PLAYBACK: Availability =
        Availability::Unavailable("no presentation clock yet; frame stepping only");
    pub const AUDIO_MONITORING: Availability =
        Availability::Unavailable("no audio device backend on this platform build");
    pub const SCOPES: Availability = Availability::Unavailable(
        "scopes are implemented in cine-color but not wired to the viewer",
    );
    pub const COLOUR_MANAGEMENT: Availability = Availability::Unavailable(
        "the display transform is not ACES-verified; see PHASE-3-STATUS.md",
    );
    pub const HDR_OUTPUT: Availability =
        Availability::Unavailable("no HDR surface configuration yet");

    /// Everything, for a capability panel that tells the truth.
    pub fn all() -> Vec<(&'static str, Availability)> {
        vec![
            ("Video display", Self::VIDEO_DISPLAY),
            ("Frame stepping", Self::FRAME_STEP),
            ("Playback", Self::PLAYBACK),
            ("Audio monitoring", Self::AUDIO_MONITORING),
            ("Scopes", Self::SCOPES),
            ("Colour management", Self::COLOUR_MANAGEMENT),
            ("HDR output", Self::HDR_OUTPUT),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every capability the shell advertises must be either genuinely working
    /// or accompanied by a reason. A silent `Unavailable` is how a fake control
    /// gets shipped.
    #[test]
    fn every_unavailable_capability_explains_itself() {
        for (name, availability) in ShellCapabilities::all() {
            if let Availability::Unavailable(reason) = availability {
                assert!(
                    reason.len() > 15,
                    "{name} is unavailable without a usable explanation: {reason:?}"
                );
            }
        }
    }

    /// The things claimed to work must be the things that actually have a
    /// backend. Playback in particular must not creep into `Working` without a
    /// presentation clock existing.
    #[test]
    fn playback_is_not_claimed_without_a_clock() {
        assert!(
            !ShellCapabilities::PLAYBACK.is_working(),
            "no presentation clock exists; claiming playback would be simulating real-time state"
        );
        assert!(!ShellCapabilities::HDR_OUTPUT.is_working());
        assert!(ShellCapabilities::VIDEO_DISPLAY.is_working());
    }
}
