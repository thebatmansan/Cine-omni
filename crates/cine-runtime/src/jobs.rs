//! The work the runtime does in the background, and how the project hears
//! about it.
//!
//! The machinery is `cine-jobs`. This module registers the runners that need
//! the project and its media, and gives each kind of work its identity, its
//! resource class and its label, so that a client asks for *what* it wants —
//! "verify this file" — and never constructs a job by hand.
//!
//! ## The runners
//!
//! * **`media.checksum`** — whole-file SHA-256, cancellable, with progress. The
//!   first run on a file *establishes* its reference checksum; every later run
//!   *verifies* against it and records the result, match or mismatch, on the
//!   asset. A mismatch never overwrites the reference: the reference is what
//!   the file was, and the point of verifying is to notice that it changed.
//! * **`media.thumbnail`** — one decoded picture, reduced to 320 pixels wide,
//!   written to the project's cache (`cache/thumbnails/`). Cache class: it may be
//!   deleted and made again, and nothing in the graph points at it.

use cine_jobs::{JobConfig, JobContext, JobId, JobRecord, JobSpec, JobSystem, Resource};
use cine_media::ContentHash;
use cine_pipeline::OmniProject;
use serde_json::{json, Value as J};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use ucg_core::{EntityId, UcgError, Value};
use ucg_schema::attrs;

/// Attribute on `asset.media` holding its checksum reference and verification.
pub const CHECKSUM_ATTR: &str = "checksum";

/// Thumbnail width in pixels.
pub const THUMBNAIL_WIDTH: u32 = 320;

/// Start the job system for a project and register every runner.
///
/// `root_dir` is where the project lives; the journal and the cache go under
/// it. A project with no directory (tests, `cine demo`) keeps no journal.
pub fn start(project: &Arc<OmniProject>, root_dir: &Path) -> std::io::Result<JobSystem> {
    let journal = project
        .disk
        .as_ref()
        .map(|_| root_dir.join("jobs").join("journal.jsonl"));
    let jobs = JobSystem::start(JobConfig::for_this_machine(journal))?;

    // Runners hold the project weakly: the project holds the job system that
    // holds the runners, and a strong reference would keep it alive forever.
    let p = Arc::downgrade(project);
    jobs.register("media.checksum", move |ctx: &JobContext, spec: &JobSpec| {
        let project = p.upgrade().ok_or("the project has closed")?;
        checksum(&project, ctx, spec)
    });
    let p = Arc::downgrade(project);
    let cache = thumbnail_cache(root_dir);
    jobs.register(
        "media.thumbnail",
        move |ctx: &JobContext, spec: &JobSpec| {
            let project = p.upgrade().ok_or("the project has closed")?;
            thumbnail(&project, &cache, ctx, spec)
        },
    );
    Ok(jobs)
}

/// Build the spec for a kind of work a client asked for.
pub fn spec_for(project: &OmniProject, kind: &str, params: &J) -> Result<JobSpec, String> {
    let media = params
        .get("media")
        .and_then(J::as_str)
        .and_then(EntityId::parse)
        .ok_or("missing entity param `media`")?;
    let name = {
        let g = project.engine.read();
        let node = g.node(media).ok_or("no such media in this project")?;
        if node.kind.as_str() != ucg_schema::kinds::ASSET_MEDIA {
            return Err(format!("{media} is not a media asset"));
        }
        node.get_str(attrs::NAME).unwrap_or("media").to_string()
    };
    match kind {
        "media.checksum" => Ok(JobSpec::new(
            kind,
            // Each verification is new work: the file may have changed since
            // the last one, which is the whole question.
            &format!("checksum:{media}:{}", project.engine.revision()),
            &format!("Verify checksum · {name}"),
            Resource::Io,
        )
        .params(json!({ "media": media }))
        .priority(5)),
        "media.thumbnail" => {
            let index = params.get("index").and_then(J::as_u64).unwrap_or(0);
            Ok(JobSpec::new(
                kind,
                &format!("thumbnail:{media}:{index}"),
                &format!("Thumbnail · {name} #{index}"),
                Resource::Cpu,
            )
            .params(json!({ "media": media, "index": index }))
            .attempts(2))
        }
        other => Err(format!(
            "unknown kind of work `{other}` (known: media.checksum, media.thumbnail)"
        )),
    }
}

fn media_param(
    project: &OmniProject,
    spec: &JobSpec,
) -> Result<(EntityId, PathBuf, Option<String>), String> {
    let media = spec.params["media"]
        .as_str()
        .and_then(EntityId::parse)
        .ok_or("the job names no media")?;
    let g = project.engine.read();
    let node = g
        .node(media)
        .ok_or("the media is no longer in the project")?;
    let path = node
        .get_str(attrs::MEDIA_PATH)
        .map(PathBuf::from)
        .ok_or("the media has no path")?;
    let hash = node.get_str(cine_media::ucg::HASH_ATTR).map(str::to_string);
    Ok((media, path, hash))
}

fn checksum(project: &OmniProject, ctx: &JobContext, spec: &JobSpec) -> Result<J, String> {
    let (media, path, _) = media_param(project, spec)?;
    let began = Instant::now();
    let mut last = Instant::now();
    let full = ContentHash::full_with_progress(&path, &mut |read, total| {
        if last.elapsed().as_millis() >= 100 {
            last = Instant::now();
            let frac = if total > 0 {
                read as f32 / total as f32
            } else {
                1.0
            };
            ctx.progress(frac, format!("{} of {} MiB", read >> 20, total >> 20));
        }
        !ctx.is_cancelled()
    })
    .map_err(|e| e.to_string())?;
    let Some(ContentHash::Full { hex, size }) = full else {
        return Ok(json!({ "cancelled": true }));
    };
    let seconds = began.elapsed().as_secs_f64();

    let reference = {
        let g = project.engine.read();
        g.node(media)
            .and_then(|n| n.get(CHECKSUM_ATTR))
            .map(Value::to_json)
    };
    let outcome = match reference.as_ref().and_then(|r| r["sha256"].as_str()) {
        None => "established",
        Some(r) if r == hex => "match",
        Some(_) => "mismatch",
    };
    let mut record =
        reference.unwrap_or_else(|| json!({ "algorithm": "sha256", "sha256": hex, "size": size }));
    record["last"] = json!({
        "result": outcome,
        "sha256": hex,
        "size": size,
        "seconds": seconds,
        "job": ctx.id(),
    });
    let label = format!("checksum {outcome}");
    project
        .engine
        .transact(&label, |tx| {
            tx.set(media, CHECKSUM_ATTR, Value::from_json(record.clone()))
                .map(|_| ())
                .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .map_err(|e| e.to_string())?;
    let mib_s = (size as f64 / (1 << 20) as f64) / seconds.max(1e-9);
    Ok(
        json!({ "result": outcome, "sha256": hex, "size": size, "seconds": seconds, "mib_per_s": mib_s }),
    )
}

fn thumbnail(
    project: &OmniProject,
    cache: &Path,
    ctx: &JobContext,
    spec: &JobSpec,
) -> Result<J, String> {
    let (_, path, hash) = media_param(project, spec)?;
    let index = spec.params["index"].as_u64().unwrap_or(0) as usize;
    let out = thumbnail_path(cache, hash.as_deref().unwrap_or("unhashed"), index);
    if out.exists() {
        return Ok(json!({ "path": out, "cached": true }));
    }
    ctx.progress(0.1, "decoding");
    let mut reader = cine_media::VideoReader::open_path(&path).map_err(|e| e.to_string())?;
    let count = reader.frame_count();
    if count == 0 {
        return Err("the media has no pictures".into());
    }
    let frame = reader
        .frame_at_index(index.min(count - 1))
        .map_err(|e| e.to_string())?
        .ok_or("no picture at that index")?;
    if ctx.is_cancelled() {
        return Ok(json!({ "cancelled": true }));
    }
    ctx.progress(0.7, "scaling");
    let (rgba, w, h, _, _) = crate::frames::rgba8(&frame)?;
    let image =
        image::RgbaImage::from_raw(w, h, rgba).ok_or("picture buffer has the wrong size")?;
    let tw = THUMBNAIL_WIDTH.min(w);
    let th = ((h as u64 * tw as u64) / w.max(1) as u64).max(1) as u32;
    // Scaled in the encoded (display) domain: a thumbnail is for recognising a
    // shot, not for judging it.
    let small = image::imageops::resize(&image, tw, th, image::imageops::FilterType::Triangle);
    let bytes = crate::frames::png(small.as_raw(), tw, th)?;
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&out, &bytes).map_err(|e| e.to_string())?;
    Ok(json!({ "path": out, "width": tw, "height": th, "bytes": bytes.len(), "cached": false }))
}

/// The thumbnail cache of a project directory.
pub fn thumbnail_cache(root_dir: &Path) -> PathBuf {
    root_dir.join("cache").join("thumbnails")
}

/// Where a thumbnail of picture `index` of the file with content hash `hash` lives.
pub fn thumbnail_path(cache: &Path, hash: &str, index: usize) -> PathBuf {
    let prefix: String = hash.chars().take(16).collect();
    cache.join(prefix).join(format!("{index}.png"))
}

/// A job record as clients see it.
pub fn record_json(r: &JobRecord) -> J {
    json!({
        "id": r.id,
        "kind": r.spec.kind,
        "label": r.spec.label,
        "state": r.state.as_str(),
        "progress": r.progress,
        "message": r.message,
        "attempts": r.attempts,
        "max_attempts": r.spec.max_attempts,
        "error": r.error,
        "output": r.output,
        "resource": r.spec.resource.as_str(),
        "priority": r.spec.priority,
        "resumed": r.resumed,
        "elapsed_ms": r.elapsed().map(|d| d.as_millis() as u64),
        "params": r.spec.params,
    })
}

/// The id of a job param.
pub fn job_id(params: &J) -> Result<JobId, String> {
    params
        .get("id")
        .and_then(J::as_u64)
        .ok_or_else(|| "missing param `id`".to_string())
}
