//! The presentation path, exercised against a real window and a real GPU.
//!
//! This drives the shipped binary in `--smoke` mode, which opens a real window,
//! configures a real surface, decodes a real frame, converts it on the GPU and
//! presents it. Anything less would test the parts and not the path.
//!
//! It is `#[ignore]`d because it needs a display and a GPU, neither of which a
//! headless CI runner has. Run it with:
//!
//! ```text
//! cargo test -p cine-shell --test presentation -- --ignored --nocapture
//! ```
//!
//! When it cannot run it says so and skips, rather than passing quietly.

use std::path::{Path, PathBuf};
use std::process::Command;

fn binary() -> Option<PathBuf> {
    // The test binary lives alongside the shell binary.
    let mut dir = std::env::current_exe().ok()?;
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    let exe = dir.join("cine-shell");
    exe.exists().then_some(exe)
}

fn media() -> Option<PathBuf> {
    let p = PathBuf::from("/home/finstein-emp/Downloads/Super_cinematic_image_to_vid.mp4");
    p.exists().then_some(p)
}

#[test]
#[ignore]
fn the_viewer_presents_real_frames_from_real_media() {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        eprintln!("SKIP: no display available");
        return;
    }
    let Some(exe) = binary() else {
        eprintln!("SKIP: the cine-shell binary has not been built");
        return;
    };
    let Some(media) = media() else {
        eprintln!("SKIP: no real media on this machine");
        return;
    };

    let dump = std::env::temp_dir().join("cine_shell_presentation_test.png");
    let _ = std::fs::remove_file(&dump);

    let out = Command::new(&exe)
        .arg(&media)
        .args(["--smoke", "10"])
        .arg("--dump")
        .arg(&dump)
        .output()
        .expect("run the viewer");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    eprintln!("{stdout}");
    assert!(
        out.status.success(),
        "the viewer exited with {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status.code()
    );
    assert!(
        stdout.contains("SMOKE OK"),
        "the viewer never confirmed presentation:\n{stdout}"
    );

    // The colour interpretation must be reported, and this asset declares none,
    // so it must be reported as assumed rather than stated as fact.
    assert!(
        stdout.contains("colour:"),
        "the viewer must report its colour interpretation"
    );
    assert!(
        stdout.contains("ASSUMED"),
        "this asset carries no matrix_coefficients; the viewer must say it guessed"
    );

    // Capabilities without a backend must be declared, not quietly omitted.
    assert!(
        stdout.contains("unavailable") && stdout.contains("Playback"),
        "playback must be declared unavailable rather than silently absent"
    );

    assert_presented_image_is_a_photograph(&dump);
}

/// The dumped frame must be a real picture, not a blank or uniform buffer.
fn assert_presented_image_is_a_photograph(path: &Path) {
    let data = std::fs::read(path).expect("the viewer wrote a frame");
    assert!(
        data.len() > 10_000,
        "presented image is implausibly small: {} bytes",
        data.len()
    );
    assert_eq!(&data[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
}
