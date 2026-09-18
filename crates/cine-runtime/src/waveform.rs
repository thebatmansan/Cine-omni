//! Making a recording's waveform, keeping it, and answering for it.
//!
//! The overview itself is [`cine_media::waveform`]: peaks decoded from the
//! source, at several zoom levels. This is the part that belongs to a project —
//! a cancellable job, a file under `derived/waveform/`, a `media.derivative`
//! node linked to the original, and the answer a timeline asks for when it
//! draws a clip.
//!
//! A waveform belongs to its original and never becomes media of its own: it
//! has no clip, no cut and no render reads it as pictures or sound. It goes
//! stale exactly when the derived rules say ([`crate::derived`]): the original's
//! content changed, the recipe changed, or its own file changed. Relinking the
//! original — moving it, renaming it — leaves it current, because the content
//! it was made from is the content the project still has.

use cine_media::waveform::{self as wav, Cache};
use cine_pipeline::OmniProject;
use serde_json::{json, Value as J};
use std::path::{Path, PathBuf};
use std::time::Instant;
use ucg_core::{EntityId, Graph, UcgError};
use ucg_schema::{attrs, kinds};

/// The job kind.
pub const WAVEFORM_KIND: &str = "media.waveform";

/// Bumped when the overview this build makes would differ.
pub const RECIPE_VERSION: &str = "waveform/1";

pub const CHANNELS_ATTR: &str = "waveform_channels";
pub const RATE_ATTR: &str = "waveform_sample_rate";
pub const FRAMES_ATTR: &str = "waveform_frames";

/// The recipe an overview records, so an older one is recognisably older.
pub fn recipe() -> String {
    format!(
        "peaks 16-bit levels {} {RECIPE_VERSION}",
        wav::LEVELS
            .iter()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join("/")
    )
}

/// Where overviews of the media with this content hash live.
pub fn waveform_dir(root: &Path, hash: &str) -> PathBuf {
    let prefix: String = hash.chars().take(16).collect();
    root.join("derived").join("waveform").join(prefix)
}

/// The overviews recorded for an original.
pub fn waveforms_of(graph: &Graph, media: EntityId) -> Vec<EntityId> {
    crate::derived::of_kind(graph, media, "waveform")
}

/// The overview a caller may use, if any: the first current one.
pub fn current_of(graph: &Graph, media: EntityId) -> Option<EntityId> {
    waveforms_of(graph, media)
        .into_iter()
        .find(|w| crate::derived::status(graph, *w, RECIPE_VERSION, false).is_current())
}

/// What the application shows about an original's waveform.
pub fn waveform_json(graph: &Graph, media: EntityId) -> J {
    let Some(id) = waveforms_of(graph, media).into_iter().next() else {
        return J::Null;
    };
    let status = crate::derived::status(graph, id, RECIPE_VERSION, false);
    let node = graph.node(id);
    json!({
        "id": id,
        "state": status.state,
        "reasons": status.reasons,
        "channels": node.and_then(|n| n.get_int(CHANNELS_ATTR)),
        "sample_rate": node.and_then(|n| n.get_int(RATE_ATTR)),
        "frames": node.and_then(|n| n.get_int(FRAMES_ATTR)),
        "file_size": node.and_then(|n| n.get_int(crate::derived::FILE_SIZE_ATTR)),
    })
}

/// Make the overview of `media` under `root`. `progress` returns `false` to
/// cancel.
pub fn generate(
    project: &OmniProject,
    root: &Path,
    media: EntityId,
    job: Option<u64>,
    progress: &mut dyn FnMut(f32, &str) -> bool,
) -> Result<J, String> {
    let began = Instant::now();
    let (path, hash, info, name) = {
        let g = project.engine.read();
        let n = g.node(media).ok_or("the media is no longer in the project")?;
        if n.kind.as_str() != kinds::ASSET_MEDIA {
            return Err(format!("{media} is not media"));
        }
        let path = n
            .get_str(attrs::MEDIA_PATH)
            .map(PathBuf::from)
            .ok_or("the media has no path")?;
        let hash = n
            .get_str(cine_media::ucg::HASH_ATTR)
            .ok_or("the media has no content hash")?
            .to_string();
        let info = cine_media::ucg::read_media(&g, media).ok_or("the media was never probed")?;
        (path, hash, info, n.name())
    };
    if !path.is_file() {
        return Err(format!("the media is offline: {}", path.display()));
    }
    let now = cine_media::sequence::identity_of(&info).map_err(|e| e.to_string())?;
    if now.hex() != hash {
        return Err(format!(
            "{} changed on disk since it was ingested; ingest it again before making its waveform",
            path.display()
        ));
    }
    let sound = info
        .first_audio()
        .ok_or_else(|| format!("`{name}` has no sound to draw"))?;
    if !sound.decodable {
        return Err(format!(
            "this build cannot decode `{name}`'s {} sound, so it cannot draw it",
            sound.codec
        ));
    }

    let recipe = recipe();
    let derivative = cine_media::identity::Derivative {
        kind: cine_media::identity::DerivativeKind::Waveform,
        source_hash: hash.clone(),
        path: waveform_dir(root, &hash).join("waveform.cinewav"),
        recipe: recipe.clone(),
    };
    let id = derivative.entity_id();
    {
        let g = project.engine.read();
        if g.node(id).is_some()
            && crate::derived::status(&g, id, RECIPE_VERSION, true).is_current()
        {
            return Ok(json!({ "waveform": id, "already_current": true, "path": derivative.path }));
        }
    }

    progress(0.0, "decoding the sound");
    let overview = wav::compute(&path, &mut |done| progress(done, "decoding the sound"))
        .map_err(|e| e.to_string())?;
    let Some(overview) = overview else {
        return Ok(json!({ "cancelled": true }));
    };

    let dir = derivative
        .path
        .parent()
        .ok_or("the waveform has nowhere to live")?
        .to_path_buf();
    let partial = dir.with_extension("partial");
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
    let file = partial.join("waveform.cinewav");
    wav::write(&file, &overview).map_err(|e| e.to_string())?;
    let manifest = json!({
        "kind": WAVEFORM_KIND,
        "original": { "media": media, "name": name, "path": path, "content_hash": hash },
        "recipe": recipe,
        "channels": overview.channels,
        "sample_rate": overview.sample_rate,
        "frames": overview.frames,
        "levels": wav::LEVELS,
        "job": job,
        "elapsed_ms": began.elapsed().as_millis() as u64,
    });
    std::fs::write(
        partial.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if dir.exists() {
        let old = dir.with_extension("replaced");
        let _ = std::fs::remove_dir_all(&old);
        std::fs::rename(&dir, &old).map_err(|e| e.to_string())?;
        std::fs::rename(&partial, &dir).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir_all(&old);
    } else {
        std::fs::rename(&partial, &dir).map_err(|e| e.to_string())?;
    }

    let written = dir.join("waveform.cinewav");
    let size = std::fs::metadata(&written).map(|m| m.len()).unwrap_or(0);
    let content = cine_media::ContentHash::fast(&written).map_err(|e| e.to_string())?;
    project
        .engine
        .transact("make waveform", |tx| {
            let id = cine_media::ucg::register_derivative(tx, media, &derivative)
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            tx.set(id, attrs::NAME, format!("{name} · waveform"))?;
            tx.set(id, crate::derived::SOURCE_HASH_ATTR, hash.as_str())?;
            tx.set(id, cine_media::ucg::HASH_ATTR, content.hex())?;
            tx.set(id, crate::derived::FILE_SIZE_ATTR, size as i64)?;
            tx.set(id, CHANNELS_ATTR, i64::from(overview.channels))?;
            tx.set(id, RATE_ATTR, i64::from(overview.sample_rate))?;
            tx.set(id, FRAMES_ATTR, overview.frames as i64)?;
            if let Some(job) = job {
                tx.set(id, crate::derived::JOB_ATTR, job as i64)?;
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    let elapsed = began.elapsed().as_secs_f64();
    Ok(json!({
        "waveform": id,
        "path": written,
        "channels": overview.channels,
        "sample_rate": overview.sample_rate,
        "frames": overview.frames,
        "bytes": size,
        "elapsed_ms": (elapsed * 1000.0) as u64,
        "seconds_per_second": (overview.frames as f64 / f64::from(overview.sample_rate)) / elapsed.max(1e-9),
    }))
}

/// The peaks covering a range of one media's sound, for drawing.
///
/// `from` and `to` are sample indices in the source's own rate — the caller
/// converts exact time once, and no rate conversion happens here. `buckets` is
/// how many columns the caller can draw; the finest level that fits is used,
/// and the answer says which, so nothing has to assume.
pub fn peaks(
    project: &OmniProject,
    cache: &Cache,
    media: EntityId,
    from: i64,
    to: i64,
    buckets: usize,
) -> Result<J, String> {
    let (id, path, status) = {
        let g = project.engine.read();
        let id = waveforms_of(&g, media)
            .into_iter()
            .next()
            .ok_or("there is no waveform of this media yet")?;
        let status = crate::derived::status(&g, id, RECIPE_VERSION, false);
        let path = g
            .node(id)
            .and_then(|n| n.get_str(attrs::MEDIA_PATH))
            .map(PathBuf::from)
            .ok_or("the waveform has no file")?;
        (id, path, status)
    };
    if !status.is_current() {
        return Err(format!(
            "the waveform is {}: {}. Make it again",
            status.state,
            status.reasons.join("; ")
        ));
    }
    let overview = cache.get(id, &path).map_err(|e| e.to_string())?;
    let window = overview.window(from, to, buckets);
    Ok(json!({
        "waveform": id,
        "channels": window.channels,
        "sample_rate": overview.sample_rate,
        "frames": overview.frames,
        "bucket": window.bucket,
        "first_sample": window.first_sample,
        // Bucket-major, channel within bucket: [min, max] per channel, as the
        // 16-bit peaks they are.
        "peaks": window
            .peaks
            .iter()
            .map(|(lo, hi)| json!([lo, hi]))
            .collect::<Vec<_>>(),
    }))
}
