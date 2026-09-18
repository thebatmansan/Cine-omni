//! How much memory one decoded picture may take, decided in one place.
//!
//! A file says how big its pictures are before any of them is read. A 60 000 ×
//! 60 000 EXR is a legal file; decoding one costs 57 GB of float RGBA, and a
//! machine asked to do it does not fail — it is killed, taking the project's
//! unsaved work with it. So the size a file *declares* is checked against a
//! budget before anything is allocated, and a picture that does not fit is
//! [`Error::Unsupported`] naming the real size and the real budget. Refusing
//! is a worse answer than decoding and a much better one than being killed.
//!
//! The budget is half of the memory this machine reports, clamped to a sane
//! range, and `CINE_PICTURE_BUDGET` (bytes) overrides it — a workstation with
//! 512 GB should not be held to a laptop's limit, and a test needs to be able
//! to ask for a small one. Nothing here reserves memory; it only decides what
//! is worth attempting.

use crate::error::{Error, Result};

/// The smallest budget this will settle on: one 8K RGBA float picture.
pub const FLOOR: u64 = 8192 * 4320 * 16;
/// The largest budget this will settle on without being told.
pub const CEILING: u64 = 16 * 1024 * 1024 * 1024;

/// Memory this machine reports as available to it, if it can tell.
///
/// Linux only, from `/proc/meminfo`'s `MemAvailable` — the kernel's own
/// estimate of what can be had without swapping. Elsewhere this returns
/// `None` and the budget is [`CEILING`]: a guess at a number would be worse
/// than saying the machine was not asked.
pub fn available_memory() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = text.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// The most memory one decoded picture may take.
pub fn picture_budget() -> u64 {
    if let Some(set) = std::env::var("CINE_PICTURE_BUDGET")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
    {
        return set;
    }
    match available_memory() {
        Some(bytes) => (bytes / 2).clamp(FLOOR, CEILING),
        None => CEILING,
    }
}

/// What one picture of `width × height` costs at `bytes_per_pixel`, or `None`
/// if the arithmetic itself overflows — which is already an answer.
pub fn picture_bytes(width: u64, height: u64, bytes_per_pixel: u64) -> Option<u64> {
    width.checked_mul(height)?.checked_mul(bytes_per_pixel)
}

/// Refuse a picture this machine should not try to decode.
///
/// `what` names the file or stream, so the refusal says which one.
pub fn check_picture(what: &str, width: u64, height: u64, bytes_per_pixel: u64) -> Result<u64> {
    let budget = picture_budget();
    let bytes = picture_bytes(width, height, bytes_per_pixel).ok_or_else(|| {
        Error::Unsupported(format!(
            "{what}: {width}x{height} at {bytes_per_pixel} bytes a pixel is larger than this \
             machine can count, let alone decode"
        ))
    })?;
    if bytes > budget {
        return Err(Error::Unsupported(format!(
            "{what}: one {width}x{height} picture needs {} to decode and this machine allows {} \
             for one picture; decoding it would end the application rather than fail. Set \
             CINE_PICTURE_BUDGET to raise the limit, or work from a smaller version.",
            human(bytes),
            human(budget)
        )));
    }
    Ok(bytes)
}

/// Bytes, for a person.
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1000.0 && u + 1 < UNITS.len() {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}
