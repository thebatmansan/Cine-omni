//! The CINE-OMNI reference viewer.
//!
//! ```text
//! cargo run -p cine-shell -- <media file>            # open a window
//! cargo run -p cine-shell -- <media file> --smoke 30 # present N frames, exit
//! cargo run -p cine-shell -- <media file> --play 5   # play N seconds, report
//! ```
//!
//! ## What drives the picture
//!
//! `cine_media::Scheduler` over `cine_media::DecodeAhead`. Decoding happens on
//! a worker thread, ahead of the playhead, into a bounded buffer. The event loop
//! does not tick at a frame rate: it sleeps until the scheduler says the picture
//! on screen has expired, takes whatever the clock says is due *then* from the
//! buffer, and puts it on the surface. When the buffer does not have it, the
//! transport reports an underrun and the loop sleeps until the worker wakes it —
//! through a winit user event, not a timer and not a spin.
//!
//! Starting playback pre-rolls: the clock is not started until a cushion of
//! pictures is decoded, and how long that took is printed. It is a visible,
//! measured step, not a silent delay.
//!
//! `--smoke` verifies the presentation path without anyone watching: it drives
//! the real window, surface and swapchain for a fixed number of frames and
//! exits non-zero if anything failed.
//!
//! `--play` is the evidence mode: it plays for a wall-clock duration and prints
//! the scheduler's counters — presented, dropped, late, underruns — the
//! decode-ahead worker's, and the time spent in each presentation stage. Those
//! numbers, not an adjective, are what this build claims.
//!
//! ## Transport
//!
//! `space` play/pause · `J`/`K`/`L` reverse/pause/forward · `←`/`→` step one
//! frame · `home` stop · `esc` quit.

use cine_shell::app::{run, ShellOptions};
use std::path::PathBuf;

fn parse_args() -> ShellOptions {
    let mut media = None;
    let mut smoke_frames = None;
    let mut play_secs = None;
    let mut ui_frames = None;
    let mut with_ui = false;
    let mut dump = None;
    let mut timeline = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--smoke" => smoke_frames = Some(it.next().and_then(|v| v.parse().ok()).unwrap_or(30)),
            "--play" => play_secs = Some(it.next().and_then(|v| v.parse().ok()).unwrap_or(5.0)),
            "--ui-smoke" => ui_frames = Some(it.next().and_then(|v| v.parse().ok()).unwrap_or(30)),
            "--with-ui" => with_ui = true,
            "--dump" => dump = it.next().map(PathBuf::from),
            "--timeline" => timeline = it.next().map(PathBuf::from),
            other if !other.starts_with("--") => media = Some(PathBuf::from(other)),
            other => eprintln!("ignoring unknown flag {other}"),
        }
    }
    ShellOptions {
        media,
        smoke_frames,
        play_secs,
        ui_frames,
        with_ui,
        dump,
        timeline,
        ..Default::default()
    }
}

fn main() {
    let args = parse_args();
    let smoke = args.smoke_frames.is_some();
    let ui_frames = args.ui_frames;
    let report = match run(args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    if let Some(failure) = report.failure {
        eprintln!("FAILED: {failure}");
        std::process::exit(1);
    }
    if smoke && report.presented == 0 {
        eprintln!("FAILED: no frames were presented");
        std::process::exit(1);
    }
    if let Some(limit) = ui_frames {
        if report.ui_drawn < limit {
            eprintln!(
                "FAILED: the workspace drew {} of {limit} frames",
                report.ui_drawn
            );
            std::process::exit(1);
        }
    }
}
