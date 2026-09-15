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
//! Since the playback scheduler landed, the window is also driven by
//! `cine_media::Scheduler`: the event loop sleeps on the scheduler's deadline
//! rather than on a frame interval, and what reaches the screen is whatever the
//! clock says is due. Playback is therefore real — and, on this build, slower
//! than real time. That is reported as [`Availability::Degraded`] rather than
//! hidden, because the honest failure of a viewer is a visible drop counter and
//! the dishonest one is a smooth picture that is not the right frame.
//!
//! There is still **no docking UI, no audio and no A/V sync**, and none is
//! simulated. A control that has no backend does not appear; see
//! [`Availability`].
//!
//! ## The rule this crate exists to keep
//!
//! `ucg-core` must never gain a dependency on any UI crate. The shell is a
//! replaceable leaf: if the toolkit choice proves wrong, this crate is replaced
//! and nothing beneath it changes. Nothing below `cine-shell` may `use` a
//! windowing or GUI crate.

pub mod app;
pub mod egui_layer;
pub mod gpu;
pub mod link;
pub mod present;
pub mod viewer;
pub mod workspace;

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
    /// Backed end to end, and meeting what its name implies.
    Working,
    /// Backed end to end, but not at the quality the name implies, with the
    /// measured shortfall shown to the user.
    ///
    /// This exists because the two-state version forced a lie either way once
    /// playback became real but slow: `Working` would have claimed real time,
    /// and `Unavailable` would have denied a feature that demonstrably runs.
    Degraded(&'static str),
    /// Deliberately absent, with the reason shown to the user.
    Unavailable(&'static str),
}

impl Availability {
    /// Whether the feature runs at all. A degraded feature does.
    pub fn is_backed(&self) -> bool {
        !matches!(self, Availability::Unavailable(_))
    }

    /// Whether the feature meets what its name implies, without qualification.
    pub fn is_working(&self) -> bool {
        matches!(self, Availability::Working)
    }

    /// What to show the user alongside, or in place of, the control.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Availability::Working => None,
            Availability::Degraded(why) | Availability::Unavailable(why) => Some(why),
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
    /// Real: the clock drives a scheduler that drives the decoder, and the
    /// event loop sleeps on the scheduler's deadline. Not real-time: measured
    /// at 21.9 fps sequential decode on this machine's 720x1280 H.264 asset in
    /// a release build, against the 24 fps the media asks for. See
    /// `cine-media/tests/playback_real_media.rs`.
    pub const PLAYBACK: Availability = Availability::Degraded(
        "playback is scheduled from the clock, but this build's decoder runs \
         below real time and drops frames; the drop counter is live",
    );
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

    /// Every capability the shell does not simply deliver must say why. A
    /// silent `Unavailable`, or a `Degraded` with no measured shortfall, is how
    /// a fake control gets shipped.
    #[test]
    fn every_qualified_capability_explains_itself() {
        for (name, availability) in ShellCapabilities::all() {
            if let Some(reason) = availability.reason() {
                assert!(
                    reason.len() > 15,
                    "{name} is qualified without a usable explanation: {reason:?}"
                );
            }
        }
    }

    /// Playback runs, and must not claim to run in real time. The measurement
    /// behind this is in `cine-media/tests/playback_real_media.rs`; if a faster
    /// decoder ever makes the claim true, that test is what promotes this.
    #[test]
    fn playback_is_backed_but_does_not_claim_real_time() {
        assert!(
            ShellCapabilities::PLAYBACK.is_backed(),
            "the scheduler drives the viewer; denying playback would be false too"
        );
        assert!(
            !ShellCapabilities::PLAYBACK.is_working(),
            "no measurement supports a real-time claim on this build"
        );
        let why = ShellCapabilities::PLAYBACK
            .reason()
            .expect("a degraded capability must state the shortfall");
        assert!(
            why.contains("below real time"),
            "the shortfall must name what is short: {why:?}"
        );
        assert!(!ShellCapabilities::HDR_OUTPUT.is_backed());
        assert!(ShellCapabilities::VIDEO_DISPLAY.is_working());
    }

    /// Audio is still absent, so nothing may imply A/V sync exists.
    #[test]
    fn nothing_claims_audio_or_sync() {
        assert!(!ShellCapabilities::AUDIO_MONITORING.is_backed());
    }
}
