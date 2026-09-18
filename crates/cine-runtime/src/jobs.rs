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
//! * **`multicam.sync`** — lines a multicam clip's angles up by what their
//!   microphones heard. Each angle's sound is decoded (up to
//!   [`SYNC_MAX_SECONDS`]), cross-correlated against the active angle's, and an
//!   angle whose best match is not clearly better than chance is left alone and
//!   reported rather than moved to a guess. The offsets are one undoable commit.
//! * **`media.waveform`** — a recording's envelope, decoded once and kept as
//!   peaks at several zoom levels. See [`crate::waveform`].
//! * **`media.relink`** — looks for the *content* of the project's media under
//!   a folder and relinks what it finds. See [`crate::relink`].
//! * **`media.proxy`** — a smaller stand-in for an original, picture for
//!   picture, registered as derived from it. See [`crate::proxy`].
//! * **`render.sequence`** — the cut written as pictures, with an optional view
//!   and burned-in captions, under `derived/render/`. See [`crate::render`].

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
    let p = Arc::downgrade(project);
    jobs.register("multicam.sync", move |ctx: &JobContext, spec: &JobSpec| {
        let project = p.upgrade().ok_or("the project has closed")?;
        sync_angles(&project, ctx, spec)
    });
    let p = Arc::downgrade(project);
    let root = root_dir.to_path_buf();
    jobs.register(
        crate::waveform::WAVEFORM_KIND,
        move |ctx: &JobContext, spec: &JobSpec| {
            let project = p.upgrade().ok_or("the project has closed")?;
            let media = spec.params["media"]
                .as_str()
                .and_then(EntityId::parse)
                .ok_or("the job names no media")?;
            crate::waveform::generate(&project, &root, media, Some(ctx.id()), &mut |f, m| {
                ctx.progress(f, m);
                !ctx.is_cancelled()
            })
        },
    );
    let p = Arc::downgrade(project);
    jobs.register(crate::relink::RELINK_KIND, move |ctx: &JobContext, spec: &JobSpec| {
        let project = p.upgrade().ok_or("the project has closed")?;
        let folder = spec.params["folder"]
            .as_str()
            .ok_or("the job names no folder to search")?;
        let only = spec.params["media"].as_str().and_then(EntityId::parse);
        let depth = spec.params["depth"]
            .as_u64()
            .map(|d| d as usize)
            .unwrap_or(crate::relink::DEFAULT_DEPTH);
        crate::relink::scan(&project, Path::new(folder), only, depth, &mut |f, m| {
            ctx.progress(f, m);
            !ctx.is_cancelled()
        })
    });
    let p = Arc::downgrade(project);
    let root = root_dir.to_path_buf();
    jobs.register(
        crate::proxy::PROXY_KIND,
        move |ctx: &JobContext, spec: &JobSpec| {
            let project = p.upgrade().ok_or("the project has closed")?;
            let media = spec.params["media"]
                .as_str()
                .and_then(EntityId::parse)
                .ok_or("the job names no media")?;
            let settings = crate::proxy::ProxySettings::from_params(&spec.params)?;
            crate::proxy::generate(&project, &root, media, settings, Some(ctx.id()), &mut |f, m| {
                ctx.progress(f, m);
                !ctx.is_cancelled()
            })
        },
    );
    let p = Arc::downgrade(project);
    let root = root_dir.to_path_buf();
    jobs.register(
        crate::render::RENDER_KIND,
        move |ctx: &JobContext, spec: &JobSpec| {
            let project = p.upgrade().ok_or("the project has closed")?;
            let request = crate::render::RenderRequest::from_params(&spec.params)?;
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let out = crate::render::render_root(&root, request.timeline)
                .join(format!("{stamp}-job{}", ctx.id()));
            crate::render::render_sequence(&project, &request, &out, &mut |f, message| {
                ctx.progress(f, message);
                !ctx.is_cancelled()
            })
        },
    );
    Ok(jobs)
}

/// Sound analysed per angle when syncing, in seconds.
pub const SYNC_MAX_SECONDS: f64 = 600.0;
/// The rate angles are compared at. Sync needs to be right to well within a
/// frame (42 ms at 24 fps); 16 kHz resolves 62.5 µs and costs a third of 48 kHz.
pub const SYNC_RATE: u32 = 16_000;
/// The least peak-to-mean correlation accepted as a match.
///
/// On its own this separates too little. Measured on real WAV files
/// (`multicam_sync.rs`): angles of the same take scored 20–22, and twelve
/// unrelated takes scored 4.6–8.7 — so the 6.0 first chosen would have lined up
/// five strangers. It is kept as a floor, and [`SYNC_MIN_AGREEMENT`] decides.
pub const SYNC_MIN_CONFIDENCE: f64 = 10.0;
/// The least agreement — Pearson correlation of the two sounds' loudness
/// envelopes at the offset found, over their overlap — accepted as a match.
/// Sound from one take rises and falls together; sound from two does not.
/// Measured: angles of one take agreed 0.9999; twelve unrelated takes ranged
/// from -0.09 to 0.43, so 0.5 would have sat too near them.
pub const SYNC_MIN_AGREEMENT: f64 = 0.8;
/// Envelope window for agreement: 50 ms at [`SYNC_RATE`].
const AGREEMENT_WINDOW: usize = 800;
/// Least overlap, in envelope windows, agreement is judged on: two seconds.
const AGREEMENT_MIN_OVERLAP: usize = 40;

/// Pearson correlation of two sounds' loudness envelopes when the candidate is
/// shifted by `offset_samples`, over the part where both exist. `None` when
/// that overlap is too short to judge.
pub fn envelope_agreement(
    reference: &[f32],
    candidate: &[f32],
    offset_samples: i64,
) -> Option<f64> {
    let r = cine_timeline::sync::rms_envelope(reference, AGREEMENT_WINDOW);
    let c = cine_timeline::sync::rms_envelope(candidate, AGREEMENT_WINDOW);
    // Candidate window j corresponds to reference window j - lag.
    let lag = offset_samples.div_euclid(AGREEMENT_WINDOW as i64);
    let pairs: Vec<(f64, f64)> = (0..r.len())
        .filter_map(|i| {
            let j = i as i64 + lag;
            (j >= 0 && (j as usize) < c.len()).then(|| (f64::from(r[i]), f64::from(c[j as usize])))
        })
        .collect();
    if pairs.len() < AGREEMENT_MIN_OVERLAP {
        return None;
    }
    let n = pairs.len() as f64;
    let (mx, my) = pairs
        .iter()
        .fold((0.0, 0.0), |(a, b), (x, y)| (a + x / n, b + y / n));
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (x, y) in &pairs {
        sxy += (x - mx) * (y - my);
        sxx += (x - mx) * (x - mx);
        syy += (y - my) * (y - my);
    }
    if sxx <= 0.0 || syy <= 0.0 {
        return Some(0.0);
    }
    Some(sxy / (sxx * syy).sqrt())
}

/// What syncing found for one angle.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct AngleSync {
    pub key: String,
    /// The media offset to record, when the match was good enough.
    pub offset: Option<ucg_core::RationalTime>,
    pub confidence: f64,
    /// How well the two sounds' loudness agrees at that offset, when judged.
    pub agreement: Option<f64>,
    /// Why it was not synced, when it was not.
    pub why_not: Option<String>,
}

/// Line angles up by sound: `reference` is the active angle's file, and each
/// candidate is `(key, file)`. Pure with respect to the project, so it is
/// testable on its own and the job only records what it returns.
pub fn sync_offsets(
    reference: &Path,
    candidates: &[(String, PathBuf)],
    keep_going: &mut dyn FnMut(f32) -> bool,
) -> Result<Vec<AngleSync>, String> {
    let total = candidates.len() as f32 + 1.0;
    let reference =
        cine_media::sound::decode_mono(reference, SYNC_RATE, SYNC_MAX_SECONDS, &mut |f| {
            keep_going(f / total)
        })
        .map_err(|e| format!("the active angle's sound: {e}"))?;
    let mut out = Vec::new();
    for (i, (key, path)) in candidates.iter().enumerate() {
        let base = (i as f32 + 1.0) / total;
        let candidate =
            match cine_media::sound::decode_mono(path, SYNC_RATE, SYNC_MAX_SECONDS, &mut |f| {
                keep_going(base + f / total)
            }) {
                Ok(c) => c,
                Err(e) => {
                    out.push(AngleSync {
                        key: key.clone(),
                        offset: None,
                        confidence: 0.0,
                        agreement: None,
                        why_not: Some(format!("its sound could not be read: {e}")),
                    });
                    continue;
                }
            };
        let found =
            cine_timeline::sync::find_offset(&reference.samples, &candidate.samples, SYNC_RATE);
        let agreement =
            envelope_agreement(&reference.samples, &candidate.samples, found.offset_samples);
        let why_not = if found.confidence < SYNC_MIN_CONFIDENCE {
            Some(format!(
                "no clear match in its sound (confidence {:.1}, at least {SYNC_MIN_CONFIDENCE} needed)",
                found.confidence
            ))
        } else {
            match agreement {
                None => {
                    Some("too little of its sound overlaps the active angle's to judge".to_string())
                }
                Some(a) if a < SYNC_MIN_AGREEMENT => Some(format!(
                    "no clear match in its sound: at the best offset its loudness agrees {a:.2}, \
                     at least {SYNC_MIN_AGREEMENT} needed"
                )),
                Some(_) => None,
            }
        };
        out.push(AngleSync {
            key: key.clone(),
            offset: why_not.is_none().then(|| {
                cine_media::sound::media_offset(&reference, &candidate, found.offset_samples)
            }),
            confidence: found.confidence,
            agreement,
            why_not,
        });
    }
    Ok(out)
}

fn sync_angles(project: &OmniProject, ctx: &JobContext, spec: &JobSpec) -> Result<J, String> {
    let clip = spec.params["clip"]
        .as_str()
        .and_then(EntityId::parse)
        .ok_or("the job names no clip")?;
    let (active, candidates) = {
        let g = project.engine.read();
        let item = cine_timeline::graph::export_item(&g, clip).map_err(|e| e.to_string())?;
        let cine_timeline::Item::Clip(c) = item else {
            return Err("that is not a clip".into());
        };
        let assets = cine_timeline::graph::angles_of_clip(&g, clip);
        let path_of = |key: &str| -> Option<PathBuf> {
            assets
                .get(key)
                .and_then(|a| g.node(*a))
                .and_then(|n| n.get_str(attrs::MEDIA_PATH))
                .map(PathBuf::from)
        };
        let active =
            path_of(&c.active_media_reference_key).ok_or("the active angle has no file")?;
        let candidates: Vec<(String, PathBuf)> = c
            .alternate_media_references
            .keys()
            .filter_map(|k| path_of(k).map(|p| (k.clone(), p)))
            .collect();
        if candidates.is_empty() {
            return Err("the clip has no other angles to sync".into());
        }
        (active, candidates)
    };
    let found = sync_offsets(&active, &candidates, &mut |f| {
        ctx.progress(f, "listening");
        !ctx.is_cancelled()
    })?;
    let offsets: Vec<(String, ucg_core::RationalTime)> = found
        .iter()
        .filter_map(|a| a.offset.map(|o| (a.key.clone(), o)))
        .collect();
    if !offsets.is_empty() {
        project
            .engine
            .transact("sync angles by sound", |tx| {
                cine_timeline::graph::set_angle_offsets(tx, clip, &offsets)
                    .map_err(|e| UcgError::Aborted(e.to_string()))
            })
            .map_err(|e| e.to_string())?;
    }
    Ok(json!({ "synced": offsets.len(), "angles": found }))
}

/// Build the spec for a kind of work a client asked for.
pub fn spec_for(project: &OmniProject, kind: &str, params: &J) -> Result<JobSpec, String> {
    if kind == crate::relink::RELINK_KIND {
        let folder = params
            .get("folder")
            .and_then(J::as_str)
            .ok_or("missing param `folder`: where to look")?;
        if !Path::new(folder).is_dir() {
            return Err(format!("{folder} is not a folder to search"));
        }
        let media = params.get("media").and_then(J::as_str);
        let label = match media {
            Some(_) => format!("Relink one · {folder}"),
            None => format!("Relink · {folder}"),
        };
        let mut spec = JobSpec::new(
            kind,
            &format!("relink:{folder}:{}:{}", media.unwrap_or("all"), project.engine.revision()),
            &label,
            Resource::Io,
        );
        spec = spec.params(json!({
            "folder": folder,
            "media": media,
            "depth": params.get("depth").and_then(J::as_u64).unwrap_or(crate::relink::DEFAULT_DEPTH as u64),
        }));
        return Ok(spec);
    }
    if kind == crate::render::RENDER_KIND {
        let request = crate::render::RenderRequest::from_params(params)?;
        let name = {
            let g = project.engine.read();
            let node = g
                .node(request.timeline)
                .ok_or("no such sequence in this project")?;
            if node.kind.as_str() != ucg_schema::kinds::NLE_TIMELINE {
                return Err(format!("{} is not a sequence", request.timeline));
            }
            node.get_str(attrs::NAME).unwrap_or("sequence").to_string()
        };
        let mut label = format!(
            "Render {} · {name}",
            match request.output {
                crate::render::Output::Movie => "movie",
                crate::render::Output::Png8 => "PNG 8-bit",
                crate::render::Output::Png16 => "PNG 16-bit",
                crate::render::Output::Exr => "EXR",
            }
        );
        if let Some(language) = &request.captions {
            label.push_str(&format!(" · {language} captions burned in"));
        }
        return Ok(JobSpec::new(
            kind,
            // A render of a later revision is a different render.
            &format!(
                "render:{}:{}:{}",
                request.timeline,
                project.engine.revision(),
                request.to_params()
            ),
            &label,
            Resource::Cpu,
        )
        .params(request.to_params()));
    }
    if kind == "multicam.sync" {
        let clip = params
            .get("clip")
            .and_then(J::as_str)
            .and_then(EntityId::parse)
            .ok_or("missing entity param `clip`")?;
        let name = {
            let g = project.engine.read();
            let node = g.node(clip).ok_or("no such clip in this project")?;
            if node.kind.as_str() != ucg_schema::kinds::NLE_CLIP {
                return Err(format!("{clip} is not a clip"));
            }
            node.get_str(attrs::NAME).unwrap_or("clip").to_string()
        };
        return Ok(JobSpec::new(
            kind,
            &format!("sync:{clip}:{}", project.engine.revision()),
            &format!("Sync angles by sound · {name}"),
            Resource::Cpu,
        )
        .params(json!({ "clip": clip })));
    }
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
        crate::proxy::PROXY_KIND => {
            let settings = crate::proxy::ProxySettings::from_params(params)?;
            Ok(JobSpec::new(
                kind,
                &format!(
                    "proxy:{media}:{}:{}:{}",
                    settings.height,
                    settings.quality,
                    project.engine.revision()
                ),
                &format!("Proxy {}p · {name}", settings.height),
                Resource::Cpu,
            )
            .params(json!({ "media": media, "height": settings.height, "quality": settings.quality })))
        }
        crate::waveform::WAVEFORM_KIND => Ok(JobSpec::new(
            kind,
            &format!("waveform:{media}"),
            &format!("Waveform · {name}"),
            Resource::Cpu,
        )
        .params(json!({ "media": media }))),
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
            "unknown kind of work `{other}` (known: media.checksum, media.thumbnail, media.proxy, media.waveform, media.relink, multicam.sync, render.sequence)"
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
    let (media, path, hash) = media_param(project, spec)?;
    let _ = &path;
    let index = spec.params["index"].as_u64().unwrap_or(0) as usize;
    let out = thumbnail_path(cache, hash.as_deref().unwrap_or("unhashed"), index);
    if out.exists() {
        return Ok(json!({ "path": out, "cached": true }));
    }
    ctx.progress(0.1, "decoding");
    let mut reader = {
        let g = project.engine.read();
        let info = cine_media::ucg::read_media(&g, media).ok_or("the media was never probed")?;
        cine_media::Pictures::open(info).map_err(|e| e.to_string())?
    };
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
