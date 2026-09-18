//! What a render depended on, and whether it still matches.
//!
//! A render records, in its manifest, one fingerprint per thing it read:
//!
//! * **the sequence** — only what the render used: every picture entry (and,
//!   for a movie with sound, every sound entry) that overlaps the rendered
//!   range, with its record time, clip, asset, file, media offset and retime.
//!   An edit elsewhere in the sequence, outside the range, changes nothing
//!   here; a trim inside it does.
//! * **each media file** used — its content hash as ingested, and its path.
//! * **the captions** burned in — those that overlap the range.
//! * **the colour** — for a render through a view: the config, the display
//!   and view the render pinned, the working space, and each used clip's input
//!   colorspace and grade.
//!
//! The render's own settings are its identity, not a dependency; the renderer
//! version is recorded and compared separately.
//!
//! Whether a render is current is answered by recomputing the same
//! fingerprints from the project as it is now and comparing them one by one,
//! and by checking each source file on disk against the hash it was rendered
//! from. Nothing else in the project is consulted, so a change to an unrelated
//! node — a script edit, another sequence, a clip outside the range — does not
//! make a render stale.
//!
//! States:
//!
//! * `current` — every fingerprint matches and every source file is unchanged.
//! * `stale` — something it depended on changed, or is gone; the reasons name
//!   what. The files are still exactly what was rendered.
//! * `invalid` — the render itself cannot be trusted: its manifest is missing
//!   or unreadable, or files it lists are missing.
//! * `failed` — a render job that did not produce a render (reported from the
//!   job system, since a failed render leaves no directory).

use crate::render::{
    frame_file, frame_range, render_root, Look, Output, RenderRequest, MOVIE_FILE, RENDER_KIND,
};
use cine_pipeline::OmniProject;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};
use sha1::{Digest, Sha1};
use std::path::Path;
use ucg_core::{EntityId, Graph, Rounding};

/// Identifies the code that produced a render. A different renderer may make
/// different pixels from the same inputs.
pub const RENDERER: &str = concat!("cine-runtime ", env!("CARGO_PKG_VERSION"), " render/2");

/// One thing a render read, fingerprinted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    pub key: String,
    pub label: String,
    pub fingerprint: String,
}

/// SHA-1 as a change detector, not as security: nothing here defends against
/// someone crafting a collision in their own project.
fn fingerprint(text: &str) -> String {
    let digest = Sha1::digest(text.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The frames `[first, last)` a request covers on `list`.
pub fn frames_of(list: &crate::playlist::EditList, request: &RenderRequest) -> (i64, i64) {
    let total = list
        .end
        .rescale_checked(list.rate, Rounding::Ceil)
        .0
        .ticks();
    request.frames.unwrap_or((0, total))
}

/// Fingerprint everything a render of `request` reads from the project.
pub fn dependencies(
    project: &OmniProject,
    graph: &Graph,
    request: &RenderRequest,
    list: &crate::playlist::EditList,
    sound: bool,
) -> Result<Vec<Dependency>, String> {
    let (first, last) = frames_of(list, request);
    let range = frame_range(list.rate, first, last);
    let pictures: Vec<(usize, &crate::playlist::Entry)> = list
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.record.overlaps(&range))
        .collect();
    let sounds: Vec<&crate::playlist::Entry> = if sound {
        list.sound
            .iter()
            .filter(|e| e.record.overlaps(&range))
            .collect()
    } else {
        Vec::new()
    };
    // An entry as far as the range sees it: the part of its record time inside
    // the range, and the media at the start of that part. Trimming a clip where
    // the range does not reach leaves this unchanged; anything that changes a
    // rendered frame changes it. A retimed clip's media depends on its whole
    // map and where it is anchored, so that is included whole.
    let describe = |e: &crate::playlist::Entry| {
        let start = e.record.start_time.max(range.start_time);
        let end = e
            .record
            .end_time_exclusive()
            .min(range.end_time_exclusive());
        let media = match &e.timing {
            Some(t) => format!("{t:?}"),
            None => format!(
                "{:?}",
                e.offset
                    .map(|o| o.checked_add(&start.checked_sub(&e.record.start_time)))
            ),
        };
        // The asset, not its path: media that moved is the same media, and the
        // media's own dependency fingerprints its content. Nor whether it is
        // online: that is where the file is today, not what was rendered.
        format!(
            "{:?}..{:?}|{:?}|{:?}|{media}|{}\n",
            start, end, e.clip, e.asset, e.keep_pitch
        )
    };
    let mut out = Vec::new();

    let mut seq = format!("rate {:?}\npictures\n", list.rate);
    if request.whole_cut {
        // "The whole cut" is itself a dependency: a longer or shorter cut is
        // a different whole.
        let total = list
            .end
            .rescale_checked(list.rate, Rounding::Ceil)
            .0
            .ticks();
        seq.push_str(&format!("whole cut of {total} frames\n"));
    }
    for (_, e) in &pictures {
        seq.push_str(&describe(e));
    }
    seq.push_str("sound\n");
    for e in &sounds {
        seq.push_str(&describe(e));
    }
    out.push(Dependency {
        key: "sequence".into(),
        label: format!("the cut of `{}` in frames {first}–{}", list.name, last - 1),
        fingerprint: fingerprint(&seq),
    });

    let mut assets: Vec<(EntityId, String)> = Vec::new();
    for e in pictures
        .iter()
        .map(|(_, e)| *e)
        .chain(sounds.iter().copied())
    {
        if let Some(a) = e.asset {
            if !assets.iter().any(|(x, _)| *x == a) {
                assets.push((a, e.name.clone()));
            }
        }
    }
    assets.sort_by_key(|(a, _)| *a);
    for (asset, name) in assets {
        let node = graph.node(asset);
        let hash = node
            .and_then(|n| n.get_str(cine_media::ucg::HASH_ATTR))
            .unwrap_or("");
        let path = node
            .and_then(|n| n.get_str(ucg_schema::attrs::MEDIA_PATH))
            .unwrap_or("");
        // By content, not by path: media that moved is the same media, and a
        // render of it is still current. The path travels in the label so the
        // file can be found and checked, and a *different* file at that path
        // changes the hash, which is the fingerprint.
        out.push(Dependency {
            key: format!("media:{asset}"),
            label: format!("the media of `{name}` ({path})"),
            fingerprint: hash.to_string(),
        });
    }

    if let Some(language) = &request.captions {
        let track = cine_timeline::graph::export_captions(graph, request.timeline, language)
            .map_err(|e| e.to_string())?;
        let used: Vec<J> = track
            .map(|t| {
                t.captions
                    .iter()
                    .filter(|c| c.record.overlaps(&range))
                    .map(|c| json!([format!("{:?}", c.record), c.lines, c.placement]))
                    .collect()
            })
            .unwrap_or_default();
        out.push(Dependency {
            key: format!("captions:{language}"),
            label: format!("the `{language}` captions"),
            fingerprint: fingerprint(&J::Array(used).to_string()),
        });
    }

    if let Look::ProjectView { display, view } = &request.look {
        let w = crate::render::wanted_views(project, graph, list, display.clone(), view.clone())?;
        let mut text = format!("{}|{}|{}|{}\n", w.config_key, w.display, w.view, w.working);
        // By clip, not by position: a clip added elsewhere shifts positions.
        for (i, e) in &pictures {
            match &w.per_entry[*i] {
                Some((input, cdl)) => text.push_str(&format!(
                    "{:?}|{input}|{}\n",
                    e.clip,
                    cine_color::view::cdl_fingerprint(cdl.as_ref())
                )),
                None => text.push_str(&format!("{:?}|none\n", e.clip)),
            }
        }
        out.push(Dependency {
            key: "colour".into(),
            label: format!("the colour: {} / {}, inputs and grades", w.display, w.view),
            fingerprint: fingerprint(&text),
        });
    }
    Ok(out)
}

/// Whether the render in `dir` is current, and why not.
pub fn status(project: &OmniProject, dir: &Path) -> J {
    let invalid = |why: String| json!({ "state": "invalid", "reasons": [why] });
    let manifest: J = match std::fs::read(dir.join("manifest.json"))
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
    {
        Ok(m) => m,
        Err(e) => return invalid(format!("its manifest cannot be read: {e}")),
    };
    let request = match RenderRequest::from_params(&manifest["request"]) {
        Ok(r) => r,
        Err(e) => {
            return with(
                manifest,
                invalid(format!("its manifest's request is not valid: {e}")),
            )
        }
    };
    // The files it says it wrote.
    match request.output {
        Output::Movie => {
            if !dir.join(MOVIE_FILE).is_file() {
                return with(manifest, invalid(format!("{MOVIE_FILE} is missing")));
            }
        }
        Output::Png8 | Output::Png16 | Output::Exr => {
            let first = manifest["frames"]["first"].as_i64().unwrap_or(0);
            let count = manifest["frames"]["count"].as_i64().unwrap_or(0);
            let name = |n: i64| match request.output {
                Output::Exr => crate::render::exr_frame_file(n),
                _ => frame_file(n),
            };
            if let Some(n) = (first..first + count).find(|n| !dir.join(name(*n)).is_file()) {
                return with(manifest, invalid(format!("{} is missing", name(n))));
            }
        }
    }
    let recorded: Vec<Dependency> =
        serde_json::from_value(manifest["dependencies"].clone()).unwrap_or_default();
    if recorded.is_empty() {
        return with(
            manifest,
            invalid("its manifest records no dependencies".into()),
        );
    }

    let mut reasons: Vec<String> = Vec::new();
    // Not every fact about a render is a reason it is out of date, and one
    // fact about a file can be either, depending on what the project claims.
    //
    // A media the project has already recorded as away — offline, or a
    // namesake at its path that relinking refused — cannot be re-read, so the
    // render cannot be remade; but nothing it was made from has changed. That
    // is a note. Calling it staleness would throw away good renders every time
    // a card is unplugged, or because a stranger's file took a name.
    //
    // A media the project still claims is online at a path whose bytes are not
    // its content is the other case entirely: the source changed under the
    // project, the render would come out different if made again now, and that
    // is staleness. The difference is not what is on disk — it is whether the
    // disk contradicts what the project says.
    let mut notes: Vec<String> = Vec::new();
    // Whether every source could be read again to remake this.
    let mut remakeable = true;
    if manifest["renderer"].as_str() != Some(RENDERER) {
        reasons.push(format!(
            "rendered by {}, not the current renderer",
            manifest["renderer"]
                .as_str()
                .unwrap_or("an unknown renderer")
        ));
    }
    let sound = !manifest["sound"].is_null();
    let current = {
        let graph = project.engine.read();
        if graph.node(request.timeline).is_none() {
            Err("the sequence no longer exists".to_string())
        } else {
            crate::playlist::edit_list(&graph, request.timeline)
                .and_then(|list| dependencies(project, &graph, &request, &list, sound))
        }
    };
    match current {
        Err(why) => reasons.push(why),
        Ok(now) => {
            for dep in &recorded {
                match now.iter().find(|d| d.key == dep.key) {
                    None => reasons.push(format!("{} is no longer part of it", dep.label)),
                    Some(d) if d.fingerprint != dep.fingerprint => {
                        reasons.push(format!("{} changed", d.label))
                    }
                    Some(_) => {}
                }
            }
            for d in now
                .iter()
                .filter(|d| !recorded.iter().any(|r| r.key == d.key))
            {
                reasons.push(format!("{} is now part of it", d.label));
            }
        }
    }
    // Files on disk, against the content they were rendered from.
    //
    // What is on disk today decides whether this render could be *made again*,
    // never whether it is out of date. Out of date is decided above, from the
    // project's own record: if the project now holds different content for
    // that media, the media dependency's fingerprint differs and the render is
    // stale. A file that is away, or a namesake sitting at its path, changes
    // neither the render nor the project's record of what it read.
    for dep in recorded.iter().filter(|d| d.key.starts_with("media:")) {
        // The fingerprint is the content hash; where the file is now, and
        // whether the project still vouches for it, is what the project says
        // now — not what it said when the render was made.
        let hash = dep.fingerprint.as_str();
        let Some((path, claimed)) = dep
            .key
            .strip_prefix("media:")
            .and_then(EntityId::parse)
            .and_then(|id| {
                let g = project.engine.read();
                let n = g.node(id)?;
                let path = n.get_str(ucg_schema::attrs::MEDIA_PATH)?;
                // The project claims this media is its content at this path
                // unless it has recorded otherwise.
                let claimed = n.get_bool(cine_media::ucg::ONLINE_ATTR) != Some(false)
                    && n.get_str(crate::relink::STATE_ATTR).is_none();
                Some((std::path::PathBuf::from(path), claimed))
            })
        else {
            reasons.push(format!("{} is no longer in the project", dep.label));
            continue;
        };
        let path = path.as_path();
        // What is on disk, against the content the render read.
        let disagreement = if !path.is_file() {
            Some(format!("{} is not at {}", dep.label, path.display()))
        } else if hash.is_empty() {
            None
        } else {
            match cine_media::ContentHash::fast(path) {
                Ok(h) if h.hex() != hash => Some(format!(
                    "the file at {} is not the content this render read",
                    path.display()
                )),
                Ok(_) => None,
                Err(e) => Some(format!("{} cannot be read: {e}", dep.label)),
            }
        };
        let Some(what) = disagreement else { continue };
        remakeable = false;
        if claimed {
            // The project vouches for content that is not there. The render
            // was made from what the project meant, and remaking it now would
            // not produce the same thing.
            reasons.push(format!(
                "{what}: the source changed on disk since it was rendered"
            ));
        } else {
            notes.push(format!(
                "{what}; the project already knows, so this render is still what it was — it \
                 cannot be made again until that media is found"
            ));
        }
    }
    reasons.dedup();
    notes.dedup();
    let state = if reasons.is_empty() {
        "current"
    } else {
        "stale"
    };
    with(
        manifest,
        json!({ "state": state, "reasons": reasons, "notes": notes, "remakeable": remakeable }),
    )
}

fn with(manifest: J, status: J) -> J {
    json!({
        "state": status["state"],
        "reasons": status["reasons"],
        // True of the render, without meaning it is out of date.
        "notes": status["notes"],
        // Whether every source it read can be read again.
        "remakeable": status["remakeable"],
        "output": manifest["request"]["output"],
        "frames": manifest["frames"],
        "width": manifest["width"],
        "height": manifest["height"],
        "fit": manifest["fit"],
        "look": manifest["look"],
        "sound": manifest["sound"],
        "captions": manifest["captions"],
        "revision": manifest["revision"],
        "renderer": manifest["renderer"],
        "elapsed_ms": manifest["elapsed_ms"],
        "precision": manifest["precision"],
    })
}

/// Every render of a sequence (or of every sequence), newest first, with its
/// state; and every render job that failed.
pub fn renders(project: &OmniProject, root: &Path, timeline: Option<EntityId>) -> J {
    let base = root.join("derived").join("render");
    let sequences: Vec<std::path::PathBuf> = match timeline {
        Some(t) => vec![render_root(root, t)],
        None => std::fs::read_dir(&base)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect()
            })
            .unwrap_or_default(),
    };
    let mut rows: Vec<(String, J)> = Vec::new();
    for seq in sequences {
        let Ok(entries) = std::fs::read_dir(&seq) else {
            continue;
        };
        for e in entries.flatten() {
            let dir = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if !dir.is_dir() || name.ends_with(".partial") {
                continue;
            }
            let mut row = status(project, &dir);
            row["name"] = json!(name);
            row["timeline"] = json!(seq.file_name().map(|n| n.to_string_lossy().into_owned()));
            row["path"] = json!(dir);
            row["movie"] = json!(dir.join(MOVIE_FILE).is_file());
            rows.push((name, row));
        }
    }
    // Directory names begin with the render's start time in seconds.
    rows.sort_by(|a, b| {
        let key = |s: &str| {
            s.split('-')
                .next()
                .and_then(|x| x.parse::<u64>().ok())
                .unwrap_or(0)
        };
        key(&b.0).cmp(&key(&a.0)).then(b.0.cmp(&a.0))
    });
    let failed: Vec<J> = project
        .jobs
        .get()
        .map(|jobs| {
            jobs.list()
                .into_iter()
                .filter(|r| r.spec.kind == RENDER_KIND && r.state == cine_jobs::JobState::Failed)
                .filter(|r| {
                    timeline
                        .is_none_or(|t| r.spec.params["timeline"].as_str() == Some(&t.to_string()))
                })
                .map(|r| {
                    json!({
                        "state": "failed",
                        "job": r.id,
                        "label": r.spec.label,
                        "reasons": [r.error],
                        "request": r.spec.params,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "renders": rows.into_iter().map(|(_, r)| r).collect::<Vec<_>>(),
        "failed": failed,
    })
}
