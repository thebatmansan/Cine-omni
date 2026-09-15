//! JSON-RPC 2.0 dispatch — the single implementation both transports share.
//!
//! `cine serve` speaks this over stdio and the runtime speaks it over
//! WebSocket. Keeping one dispatch means a method cannot exist on one transport
//! and not the other, and cannot behave differently between them.

use cine_pipeline::OmniProject;
use serde_json::{json, Value as J};
use ucg_core::prelude::*;
use ucg_schema::{attrs, kinds};

pub type RpcResult = std::result::Result<J, String>;

/// A parsed request. `id` absent means a notification, which takes no reply.
pub struct Request {
    pub id: Option<J>,
    pub method: String,
    pub params: J,
}

impl Request {
    pub fn parse(line: &str) -> std::result::Result<Request, String> {
        let v: J = serde_json::from_str(line).map_err(|e| e.to_string())?;
        let method = v
            .get("method")
            .and_then(J::as_str)
            .ok_or_else(|| "missing `method`".to_string())?
            .to_string();
        Ok(Request {
            id: v.get("id").cloned(),
            method,
            params: v.get("params").cloned().unwrap_or(J::Null),
        })
    }
}

pub fn ok_response(id: &J, result: J) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

pub fn error_response(id: &J, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// A job notification: every state change, and progress while running.
pub fn job_notification(record: &cine_jobs::JobRecord) -> String {
    json!({
        "jsonrpc": "2.0",
        "method": "job",
        "params": crate::jobs::record_json(record),
    })
    .to_string()
}

/// A shared-session notification, as pushed to every connected client.
pub fn session_notification(event: &cine_pipeline::session::SessionEvent) -> String {
    match event {
        cine_pipeline::session::SessionEvent::Focus(t) => json!({
            "jsonrpc": "2.0",
            "method": "focus",
            "params": { "timeline": t },
        })
        .to_string(),
    }
}

/// A change notification, as pushed to every connected client.
pub fn change_notification(cs: &ucg_core::ChangeSet) -> String {
    json!({
        "jsonrpc": "2.0",
        "method": "changed",
        "params": {
            "revision": cs.revision(),
            "label": cs.delta.label,
            "depth": cs.depth(),
            "touched": cs.touched,
            "dirty": cs.dirty.len(),
        }
    })
    .to_string()
}

/// What this build can actually do, in the words of the systems that know.
///
/// Two sources, neither of them invented here. Codec support comes from
/// `cine_media::capability::REGISTRY`, which carries its own evidence and
/// refuses to name a hardware path it does not have. The rest are facts about
/// *this* runtime and this transport, and they are stated rather than implied:
/// a browser cannot receive GPU textures, so the thing it can show is a decoded
/// picture at a time, and it must say so.
///
/// States are deliberately the same four the rest of the project uses:
/// `verified`, `degraded`, `unavailable`, `refused`.
fn capabilities() -> J {
    use cine_media::capability::{Verification, REGISTRY};

    let codecs: Vec<J> = REGISTRY
        .iter()
        .map(|c| {
            let (state, detail) = match c.verification {
                Verification::Verified { evidence } => ("verified", evidence),
                Verification::Unverified { note } => ("degraded", note),
                Verification::Refused { reason } => ("refused", reason),
            };
            json!({
                "id": format!("codec.{}{}", c.codec, c.profile.map(|p| format!(".{p}")).unwrap_or_default()),
                "name": c.to_string(),
                "group": "Decoding",
                "state": state,
                "detail": detail,
            })
        })
        .collect();

    let session = vec![
        json!({
            "id": "project.graph",
            "name": "Project graph, undo and operation log",
            "group": "Project",
            "state": "verified",
            "detail": "One engine per project; every commit is broadcast to every client",
        }),
        json!({
            "id": "project.live_sync",
            "name": "Live state across clients",
            "group": "Project",
            "state": "verified",
            "detail": "The runtime owns the only engine; clients hold projections, never state",
        }),
        json!({
            "id": "editorial.timeline",
            "name": "Timeline, tracks, clips, track state",
            "group": "Editorial",
            "state": "verified",
            "detail": "Read from the graph; lock, mute, solo and target commit through it",
        }),
        json!({
            "id": "editorial.operations",
            "name": "Trim, roll, slip, slide, split, lift, extract, insert, overwrite, replace, join",
            "group": "Editorial",
            "state": "verified",
            "detail": "Each edit is one undoable transaction carried to linked material; \
                       tested over the protocol (editorial_protocol.rs)",
        }),
        json!({
            "id": "project.jobs",
            "name": "Background work: checksum verification, thumbnails",
            "group": "Project",
            "state": "verified",
            "detail": "Prioritised, resource-bounded, cancellable, retried and journalled; \
                       interrupted work resumes after a restart. Proxies, renders and \
                       transcodes are not yet kinds of work",
        }),
        json!({
            "id": "project.impact",
            "name": "What a change affected, by department",
            "group": "Project",
            "state": "verified",
            "detail": "Read from the committed deltas of the change and its reactor cascade; \
                       nothing is inferred",
        }),
        json!({
            "id": "script.lines",
            "name": "Screenplay reading and line editing",
            "group": "Script",
            "state": "degraded",
            "detail": "Scenes and elements read from the graph; dialogue text is editable and \
                       cascades. Writing new scenes, pagination and revisions are not built",
        }),
        json!({
            "id": "media.preview",
            "name": "Decoded picture preview",
            "group": "Media",
            "state": "degraded",
            "detail": "Real decoding, one picture per request, encoded as PNG. \
                       Frame-accurate stepping, not real-time playback",
        }),
        json!({
            "id": "media.playback",
            "name": "Real-time playback in the browser",
            "group": "Media",
            "state": "unavailable",
            "detail": "The verified playback path decodes to GPU textures and never leaves the \
                       GPU. A browser cannot receive those. Real-time playback of the current \
                       sequence runs in the native window cine start opens",
        }),
        json!({
            "id": "audio.device",
            "name": "Audio output device",
            "group": "Audio",
            "state": if cine_audio_io::AlsaSink::available().is_ok() { "verified" } else { "unavailable" },
            "detail": if cine_audio_io::AlsaSink::available().is_ok() {
                "ALSA playback through the system default device, loaded at runtime; measured \
                 consuming 48 kHz within 61 ppm of the system clock, and a picture clock paced \
                 by it holding within 0.3 ms"
            } else {
                "libasound could not be loaded on this system"
            },
        }),
        json!({
            "id": "audio.monitoring",
            "name": "Audio monitoring and A/V sync",
            "group": "Audio",
            "state": "unavailable",
            "detail": "The device works (see audio.device), but the cut's own sound is not \
                       yet decoded, mixed and played with the picture",
        }),
        json!({
            "id": "colour.management",
            "name": "Colour management",
            "group": "Colour",
            "state": "unavailable",
            "detail": "The display transform is not ACES-verified; see PHASE-3-STATUS.md",
        }),
        json!({
            "id": "colour.scopes",
            "name": "Scopes",
            "group": "Colour",
            "state": "unavailable",
            "detail": "Implemented in cine-color, not wired to any viewer",
        }),
    ];

    json!({ "capabilities": session.into_iter().chain(codecs).collect::<Vec<_>>() })
}

fn entity(params: &J, key: &str) -> std::result::Result<EntityId, String> {
    params
        .get(key)
        .and_then(J::as_str)
        .and_then(EntityId::parse)
        .ok_or_else(|| format!("missing entity param `{key}`"))
}

fn str_param<'a>(params: &'a J, key: &str) -> std::result::Result<&'a str, String> {
    params
        .get(key)
        .and_then(J::as_str)
        .ok_or_else(|| format!("missing param `{key}`"))
}

pub fn first_document(p: &OmniProject) -> Option<EntityId> {
    p.engine
        .read()
        .of_kind(kinds::SCRIPT_DOCUMENT)
        .first()
        .map(|n| n.id)
}

/// The whole project as text: statistics, every scene's derived artefacts, and
/// the schedule. Used by the CLI and by the `report` method.
pub fn full_report(p: &OmniProject) -> String {
    let mut out = String::new();
    let stats = p.engine.stats();
    out.push_str(&format!(
        "revision {} — {} nodes, {} edges, {} reactors\n",
        stats.revision,
        stats.nodes,
        stats.edges,
        p.engine.reactor_names().len()
    ));
    let mut kinds: Vec<_> = stats.by_kind.iter().collect();
    kinds.sort();
    for (k, n) in kinds {
        out.push_str(&format!("  {k:<28}{n:>6}\n"));
    }
    if let Some(doc) = first_document(p) {
        for scene in p.scenes(doc) {
            out.push_str(&p.scene_report(scene));
        }
        let g = p.engine.read();
        if let Some(schedule) = cine_logistics::graph::read_schedule(&g, doc) {
            out.push_str("\nSCHEDULE\n");
            out.push_str(&schedule.to_markdown());
        }
    }
    out
}

/// Render one of the project's export formats.
pub fn export(
    p: &OmniProject,
    what: &str,
    scene: Option<usize>,
) -> std::result::Result<String, String> {
    let doc = first_document(p).ok_or_else(|| "no screenplay imported".to_string())?;
    Ok(match what {
        "fountain" => p.export_fountain(doc).map_err(|x| x.to_string())?,
        "fdx" => p.export_fdx(doc).map_err(|x| x.to_string())?,
        "usda" => {
            let s = *p
                .scenes(doc)
                .get(scene.unwrap_or(0))
                .ok_or_else(|| "scene index out of range".to_string())?;
            p.export_usda(s).map_err(|x| x.to_string())?
        }
        "otio" => {
            let t = p
                .engine
                .read()
                .of_kind(kinds::NLE_TIMELINE)
                .first()
                .map(|n| n.id)
                .ok_or_else(|| "no timeline imported".to_string())?;
            p.export_otio(t).map_err(|x| x.to_string())?
        }
        "breakdown" => {
            let g = p.engine.read();
            cine_logistics::breakdown::to_markdown(&cine_logistics::breakdown::breakdown_document(
                &g, doc,
            ))
        }
        "schedule" => {
            let g = p.engine.read();
            cine_logistics::graph::read_schedule(&g, doc)
                .map(|s| s.to_markdown())
                .unwrap_or_else(|| "no schedule".into())
        }
        "callsheets" => {
            let g = p.engine.read();
            let schedule = EntityId::derived(doc, kinds::LOGISTICS_SCHEDULE);
            let mut out = String::new();
            for day in g.children(schedule) {
                if let Some(sheet) = cine_logistics::graph::read_call_sheet(&g, day) {
                    out.push_str(&sheet.to_markdown());
                    out.push('\n');
                }
            }
            out
        }
        other => return Err(format!("unknown export `{other}`")),
    })
}

pub fn cascade_json(group: &UndoGroup) -> J {
    json!({
        "label": group.label,
        "deltas": group.deltas.iter().map(|d| json!({
            "revision": d.revision, "label": d.label, "depth": d.depth, "ops": d.ops.len()
        })).collect::<Vec<_>>()
    })
}

/// A cascade result with its impact, for every method that changes the project.
fn with_impact(p: &OmniProject, group: &UndoGroup) -> J {
    let mut out = cascade_json(group);
    let g = p.engine.read();
    out["impact"] = crate::impact::of_group(&g, group);
    out
}

/// A delta that is not in the undo history — an undo or redo commit — and the
/// cascade it caused, rebuilt from the session log.
fn group_from_log(p: &OmniProject, revision: u64) -> Option<UndoGroup> {
    let log = p.engine.log();
    let root = log.iter().find(|d| d.revision == revision)?.clone();
    let mut revisions = vec![root.revision];
    let mut deltas = vec![root.clone()];
    for d in log.iter().filter(|d| d.revision > root.revision) {
        if d.cause.is_some_and(|c| revisions.contains(&c)) {
            revisions.push(d.revision);
            deltas.push(d.clone());
        }
    }
    Some(UndoGroup {
        label: root.label.clone(),
        deltas,
    })
}

fn op_label(op: &cine_timeline::EditOp) -> &'static str {
    use cine_timeline::EditOp as E;
    match op {
        E::RippleTrim { .. } => "ripple trim",
        E::Roll { .. } => "roll",
        E::Slip { .. } => "slip",
        E::Slide { .. } => "slide",
        E::SplitAt { .. } => "split",
        E::Extract { .. } => "extract",
        E::Lift { .. } => "lift",
        E::Insert { .. } => "insert",
        E::Overwrite { .. } => "overwrite",
        E::Replace { .. } => "replace",
        E::Join { .. } => "join",
    }
}

/// Dispatch one call against the shared project.
///
/// Read guards are scoped inside each arm: the engine's lock is not reentrant,
/// and holding one across a `transact` deadlocks.
pub fn dispatch(p: &OmniProject, method: &str, params: &J) -> RpcResult {
    dispatch_with(p, method, params, None)
}

/// Session facts a bare project does not know: how many clients are attached
/// and how long the runtime has been up.
#[derive(Clone, Copy, Debug, Default)]
pub struct SessionFacts {
    pub clients: usize,
    pub uptime_s: u64,
}

/// Dispatch, with optional session facts for the methods that report them.
pub fn dispatch_with(
    p: &OmniProject,
    method: &str,
    params: &J,
    session: Option<SessionFacts>,
) -> RpcResult {
    let session = session.unwrap_or_default();
    Ok(match method {
        // -- what the project is ------------------------------------------
        "project" => {
            let stats = p.engine.stats();
            let g = p.engine.read();
            let path = p.disk.as_ref().map(|d| d.path.clone()).unwrap_or_default();
            crate::projection::project(&g, &stats, &path, session.clients, session.uptime_s)
        }
        "capabilities" => capabilities(),

        // -- editorial ------------------------------------------------------
        "timelines" => {
            let g = p.engine.read();
            crate::projection::timelines(&g)
        }
        "timeline" => {
            let id = entity(params, "id")?;
            let g = p.engine.read();
            crate::projection::timeline(&g, id)?
        }
        "track_state" => {
            let track = entity(params, "track")?;
            let key = str_param(params, "key")?.to_string();
            let on = params.get("value").and_then(J::as_bool).unwrap_or(false);
            let state = {
                let g = p.engine.read();
                cine_timeline::graph::track_state(&g, track).map_err(|x| x.to_string())?
            };
            let mut next = state;
            match key.as_str() {
                "locked" => next.locked = on,
                "muted" => next.muted = on,
                "solo" => next.solo = on,
                "targeted" => next.targeted = on,
                other => return Err(format!("unknown track state `{other}`")),
            }
            let group = p
                .engine
                .transact(&format!("track {key}"), |tx| {
                    cine_timeline::graph::set_track_state(tx, track, next)
                        .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))
                })
                .map_err(|x| x.to_string())?;
            cascade_json(&group)
        }
        "import_otio" => {
            let text = str_param(params, "text")?;
            let key = params
                .get("key")
                .and_then(J::as_str)
                .unwrap_or("timeline.otio");
            let id = p.import_otio(text, key).map_err(|x| x.to_string())?;
            json!({ "id": id })
        }

        "edit" => {
            let track = entity(params, "track")?;
            let op: cine_timeline::EditOp =
                serde_json::from_value(params.get("op").cloned().ok_or("missing param `op`")?)
                    .map_err(|e| format!("invalid edit op: {e}"))?;
            let linked = params.get("linked").and_then(J::as_bool).unwrap_or(true);
            let mut outcome = None;
            let label = format!("edit: {}", op_label(&op));
            let group = p
                .engine
                .transact(&label, |tx| {
                    outcome = Some(
                        cine_timeline::perform_edit(tx, track, op, linked)
                            .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))?,
                    );
                    Ok(())
                })
                .map_err(|x| x.to_string())?;
            let o = outcome.ok_or("the edit did not run")?;
            let g = p.engine.read();
            json!({
                "outcome": {
                    "label": o.label,
                    "requested": o.requested.map(crate::projection::time),
                    "applied": o.applied.map(crate::projection::time),
                    "clamped": o.requested != o.applied,
                    "item": o.item,
                    "changed_tracks": o.changed_tracks,
                    "linked_clips": o.linked_clips,
                },
                "cascade": cascade_json(&group),
                "impact": crate::impact::of_group(&g, &group),
            })
        }
        "new_timeline" => {
            let name = params
                .get("name")
                .and_then(J::as_str)
                .unwrap_or("Sequence")
                .to_string();
            let num = params.get("rate_num").and_then(J::as_u64).unwrap_or(24);
            let den = params.get("rate_den").and_then(J::as_u64).unwrap_or(1);
            let rate = u32::try_from(num)
                .ok()
                .zip(u32::try_from(den).ok())
                .and_then(|(n, d)| ucg_core::Rate::new(n, d))
                .ok_or_else(|| format!("invalid rate {num}/{den}"))?;
            let root = p.root;
            let mut id = EntityId::nil();
            let group = p
                .engine
                .transact(&format!("new sequence {name}"), |tx| {
                    id = cine_timeline::graph::create_sequence(tx, root, &name, rate)
                        .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))?;
                    Ok(())
                })
                .map_err(|x| x.to_string())?;
            let mut out = with_impact(p, &group);
            out["id"] = json!(id);
            out
        }
        "place" => {
            let tracks: Vec<EntityId> = params
                .get("tracks")
                .and_then(J::as_array)
                .ok_or("missing param `tracks`")?
                .iter()
                .map(|v| {
                    v.as_str()
                        .and_then(EntityId::parse)
                        .ok_or("`tracks` must be entity ids")
                })
                .collect::<std::result::Result<_, _>>()?;
            let time: RationalTime =
                serde_json::from_value(params.get("time").cloned().ok_or("missing param `time`")?)
                    .map_err(|e| format!("invalid time: {e}"))?;
            let spec: cine_timeline::ClipSpec =
                serde_json::from_value(params.get("clip").cloned().ok_or("missing param `clip`")?)
                    .map_err(|e| format!("invalid clip: {e}"))?;
            let overwrite = params.get("mode").and_then(J::as_str) == Some("overwrite");
            let mut placed = Vec::new();
            let group = p
                .engine
                .transact(
                    if overwrite {
                        "edit: overwrite"
                    } else {
                        "edit: insert"
                    },
                    |tx| {
                        placed = cine_timeline::graph::place_material(
                            tx, &tracks, time, &spec, overwrite,
                        )
                        .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))?;
                        Ok(())
                    },
                )
                .map_err(|x| x.to_string())?;
            let mut out = with_impact(p, &group);
            out["clips"] = json!(placed);
            out
        }
        "jobs" => {
            let jobs = p
                .jobs
                .get()
                .ok_or("background work is not running in this process")?;
            let limit = params.get("limit").and_then(J::as_u64).unwrap_or(100) as usize;
            J::Array(
                jobs.list()
                    .iter()
                    .take(limit)
                    .map(crate::jobs::record_json)
                    .collect(),
            )
        }
        "job_submit" => {
            let jobs = p
                .jobs
                .get()
                .ok_or("background work is not running in this process")?;
            let kind = str_param(params, "kind")?;
            let spec = crate::jobs::spec_for(p, kind, params)?;
            let id = jobs.submit(spec);
            crate::jobs::record_json(&jobs.get(id).ok_or("the job vanished")?)
        }
        "job_cancel" | "job_retry" => {
            let jobs = p
                .jobs
                .get()
                .ok_or("background work is not running in this process")?;
            let id = crate::jobs::job_id(params)?;
            let done = if method == "job_cancel" {
                jobs.cancel(id)
            } else {
                jobs.retry(id)
            };
            if !done {
                return Err(format!(
                    "job {id} cannot be {} in its current state",
                    if method == "job_cancel" {
                        "cancelled"
                    } else {
                        "retried"
                    }
                ));
            }
            crate::jobs::record_json(&jobs.get(id).ok_or("no such job")?)
        }
        "focus" => {
            // With `timeline`, open that sequence in every surface; `null`
            // closes it. Without, report what is open.
            if let Some(v) = params.get("timeline") {
                let want = match v {
                    J::Null => None,
                    other => {
                        let id = other
                            .as_str()
                            .and_then(EntityId::parse)
                            .ok_or("`timeline` must be an entity id or null")?;
                        let g = p.engine.read();
                        if !g
                            .node(id)
                            .is_some_and(|n| n.kind.as_str() == kinds::NLE_TIMELINE)
                        {
                            return Err(format!("{id} is not a sequence in this project"));
                        }
                        Some(id)
                    }
                };
                p.session.set_focus(want);
            }
            json!({ "timeline": p.session.focus() })
        }
        "edit_list" => {
            let timeline = entity(params, "timeline")?;
            let g = p.engine.read();
            crate::projection::edit_list(&g, timeline)?
        }
        "picture_at" => {
            let timeline = entity(params, "timeline")?;
            let at: RationalTime =
                serde_json::from_value(params.get("time").cloned().ok_or("missing param `time`")?)
                    .map_err(|e| format!("invalid time: {e}"))?;
            let g = p.engine.read();
            crate::projection::picture_at(&g, timeline, at)?
        }
        "link" => {
            let clips: Vec<EntityId> = params
                .get("clips")
                .and_then(J::as_array)
                .ok_or("missing param `clips`")?
                .iter()
                .map(|v| {
                    v.as_str()
                        .and_then(EntityId::parse)
                        .ok_or("`clips` must be entity ids")
                })
                .collect::<std::result::Result<_, _>>()?;
            let group_name = params.get("group").and_then(J::as_str).map(str::to_string);
            let group = p
                .engine
                .transact(
                    if group_name.is_some() {
                        "link clips"
                    } else {
                        "unlink clips"
                    },
                    |tx| {
                        for c in &clips {
                            cine_timeline::graph::set_clip_link(tx, *c, group_name.as_deref())
                                .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))?;
                        }
                        Ok(())
                    },
                )
                .map_err(|x| x.to_string())?;
            with_impact(p, &group)
        }
        "bind_clip" | "unbind_clip" => {
            let clip = entity(params, "clip")?;
            let scene = entity(params, "scene")?;
            let bind = method == "bind_clip";
            let group = p
                .engine
                .transact(
                    if bind {
                        "bind clip to scene"
                    } else {
                        "unbind clip from scene"
                    },
                    |tx| {
                        let r = if bind {
                            cine_timeline::bind_clip_to_scene(tx, clip, scene)
                        } else {
                            cine_timeline::graph::unbind_clip_from_scene(tx, clip, scene)
                        };
                        r.map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))
                    },
                )
                .map_err(|x| x.to_string())?;
            with_impact(p, &group)
        }
        "conform_done" => {
            let clip = entity(params, "clip")?;
            let group = p
                .engine
                .transact("mark conformed", |tx| {
                    cine_timeline::clear_conform(tx, clip)
                        .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))
                })
                .map_err(|x| x.to_string())?;
            with_impact(p, &group)
        }
        "script" => {
            let g = p.engine.read();
            crate::projection::script(&g)
        }
        "impact" => {
            let history = p.engine.history();
            let g = p.engine.read();
            let revision = params.get("revision").and_then(J::as_u64);
            let group = match revision {
                Some(r) => crate::impact::find_group(&history, r)
                    .cloned()
                    .or_else(|| group_from_log(p, r)),
                None => history.undo.last().cloned(),
            };
            match group {
                Some(group) => crate::impact::of_group(&g, &group),
                None => J::Null,
            }
        }
        "history" => {
            let limit = params.get("limit").and_then(J::as_u64).unwrap_or(50) as usize;
            crate::impact::history(&p.engine.history(), limit)
        }

        // -- media ----------------------------------------------------------
        "media" => {
            let g = p.engine.read();
            crate::projection::media(&g)
        }
        "ingest" => {
            let path = std::path::PathBuf::from(str_param(params, "path")?);
            if !path.exists() {
                return Err(format!("no file at {}", path.display()));
            }
            let root = p.root;
            let mut id = ucg_core::EntityId::nil();
            p.engine
                .transact("ingest media", |tx| {
                    id = cine_media::ucg::ingest(tx, root, &path)
                        .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))?;
                    Ok(())
                })
                .map_err(|x| x.to_string())?;
            json!({ "id": id })
        }

        // -- the production chain -------------------------------------------
        "structure" => {
            let g = p.engine.read();
            crate::projection::structure(&g)
        }

        "stats" => serde_json::to_value(p.engine.stats()).map_err(|x| x.to_string())?,
        "node" => {
            let id = entity(params, "id")?;
            let g = p.engine.read();
            let n = g.require(id).map_err(|x| x.to_string())?;
            json!({
                "id": n.id,
                "kind": n.kind,
                "attrs": n.attrs.iter().map(|(k, v)| (k.clone(), v.to_json()))
                    .collect::<serde_json::Map<_, _>>(),
                "children": g.children(id),
                "edges_out": g.out_edges(id),
                "edges_in": g.in_edges(id),
            })
        }
        "kind" => {
            let kind = str_param(params, "kind")?;
            let g = p.engine.read();
            serde_json::to_value(
                g.of_kind(kind)
                    .iter()
                    .map(|n| json!({ "id": n.id, "name": n.name() }))
                    .collect::<Vec<_>>(),
            )
            .map_err(|x| x.to_string())?
        }
        "scenes" => {
            let doc = first_document(p).ok_or("no screenplay imported")?;
            let g = p.engine.read();
            serde_json::to_value(
                p.scenes(doc)
                    .into_iter()
                    .filter_map(|s| {
                        let n = g.node(s)?;
                        Some(json!({
                            "id": s,
                            "heading": n.get_str(attrs::HEADING),
                            "number": n.get_str(attrs::SCENE_NUMBER),
                            "words": n.get_int(attrs::WORDS),
                            "eighths": n.get_int(attrs::PAGE_EIGHTHS),
                            "est_duration_s": n.get_float(attrs::EST_DURATION_S),
                            "lines": p.dialogue_lines(s),
                        }))
                    })
                    .collect::<Vec<_>>(),
            )
            .map_err(|x| x.to_string())?
        }
        "report" => J::String(full_report(p)),
        "scene_report" => J::String(p.scene_report(entity(params, "scene")?)),
        "export" => {
            let what = str_param(params, "what")?;
            let scene = params.get("scene").and_then(J::as_u64).map(|v| v as usize);
            J::String(export(p, what, scene)?)
        }
        "set" => {
            let id = entity(params, "id")?;
            let key = str_param(params, "key")?.to_string();
            let value = Value::from_json(params.get("value").cloned().unwrap_or(J::Null));
            let group = p
                .engine
                .transact(&format!("set {key}"), |tx| tx.set(id, &key, value))
                .map_err(|x| x.to_string())?;
            with_impact(p, &group)
        }
        "edit_dialogue" => {
            let line = entity(params, "line")?;
            let text = str_param(params, "text")?;
            with_impact(p, &p.edit_dialogue(line, text).map_err(|x| x.to_string())?)
        }
        "import_fountain" => {
            let text = str_param(params, "text")?;
            let key = params
                .get("key")
                .and_then(J::as_str)
                .unwrap_or("script.fountain");
            serde_json::to_value(p.import_fountain(text, key).map_err(|x| x.to_string())?)
                .map_err(|x| x.to_string())?
        }
        "undo" => with_impact(p, &p.engine.undo().map_err(|x| x.to_string())?),
        "redo" => with_impact(p, &p.engine.redo().map_err(|x| x.to_string())?),
        "save" => {
            p.save().map_err(|x| x.to_string())?;
            J::Bool(true)
        }
        "validate" => {
            serde_json::to_value(p.engine.read().validate()).map_err(|x| x.to_string())?
        }
        "shutdown" => J::Bool(true),
        other => return Err(format!("unknown method `{other}`")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cine_pipeline::ProjectSetup;

    fn project() -> OmniProject {
        OmniProject::new(ProjectSetup::default()).expect("project")
    }

    #[test]
    fn a_request_without_an_id_is_a_notification() {
        let n = Request::parse(r#"{"jsonrpc":"2.0","method":"ping"}"#).unwrap();
        assert!(n.id.is_none(), "a notification takes no reply");
        let c = Request::parse(r#"{"jsonrpc":"2.0","id":4,"method":"stats"}"#).unwrap();
        assert_eq!(c.id, Some(json!(4)));
    }

    #[test]
    fn malformed_input_is_reported_rather_than_ignored() {
        assert!(Request::parse("not json").is_err());
        assert!(
            Request::parse(r#"{"jsonrpc":"2.0","id":1}"#).is_err(),
            "no method"
        );
    }

    #[test]
    fn an_unknown_method_names_itself_in_the_error() {
        let p = project();
        let err = dispatch(&p, "teleport", &J::Null).unwrap_err();
        assert!(
            err.contains("teleport"),
            "the error must name the method: {err}"
        );
    }

    #[test]
    fn a_missing_entity_parameter_names_the_parameter() {
        let p = project();
        let err = dispatch(&p, "node", &json!({})).unwrap_err();
        assert!(err.contains("id"), "{err}");
    }

    #[test]
    fn stats_reports_the_engine_revision() {
        let p = project();
        let before = dispatch(&p, "stats", &J::Null).unwrap();
        p.engine
            .transact("touch", |tx| tx.set(p.root, "notes", "x"))
            .unwrap();
        let after = dispatch(&p, "stats", &J::Null).unwrap();
        assert!(
            after["revision"].as_u64() > before["revision"].as_u64(),
            "a commit must move the revision the clients track"
        );
    }

    /// The dispatch must not hold a read guard across a transaction; if it did,
    /// this would deadlock rather than fail.
    #[test]
    fn a_write_method_completes_without_deadlocking() {
        let p = project();
        let result = dispatch(
            &p,
            "set",
            &json!({ "id": p.root.to_string(), "key": "notes", "value": "hello" }),
        )
        .unwrap();
        assert!(result["deltas"].as_array().is_some_and(|d| !d.is_empty()));
    }

    #[test]
    fn responses_carry_the_request_id_back() {
        assert!(ok_response(&json!(9), json!(true)).contains("\"id\":9"));
        let err = error_response(&json!("abc"), -32000, "boom");
        assert!(err.contains("\"id\":\"abc\"") && err.contains("boom"));
    }
}
