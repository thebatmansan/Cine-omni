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
        cine_pipeline::session::SessionEvent::View(v) => json!({
            "jsonrpc": "2.0",
            "method": "view",
            "params": v.as_ref().map(|v| json!({ "display": v.display, "view": v.view })),
        })
        .to_string(),
        cine_pipeline::session::SessionEvent::Media(m) => json!({
            "jsonrpc": "2.0",
            "method": "media_preference",
            "params": { "preference": m.as_str() },
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
    use cine_media::capability::{Verification, MEDIA_PATHS, REGISTRY};

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

    // Whole media paths, from the same registry the codecs come from: what
    // exists, how far, and what is deliberately not attempted. Nothing here is
    // written in this function, so no interface can claim more than the table.
    let paths: Vec<J> = MEDIA_PATHS
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "group": format!("Media · {}", p.area),
                "state": p.state.label(),
                "detail": match p.evidence {
                    Some(e) => format!("{} — {e}", p.detail),
                    None => p.detail.to_string(),
                },
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
            "id": "production.planning",
            "name": "Breakdown, stripboard, call sheets, shot lists",
            "group": "Production",
            "state": "degraded",
            "detail": "Derived from the screenplay and kept current by the logistics reactor; \
                       scheduling constraints are editable and reschedule the film. Strips \
                       cannot be moved by hand, breakdown elements cannot be tagged by hand, \
                       and there is no budget",
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
            "state": "degraded",
            "detail": "In the native window the sequence's sound is decoded, mixed and played, \
                       and paces the picture (measured -3.5 ms median, 3 ms spread, by the \
                       device's reported delay). Not in the browser. Retimed clips play as \
                       varispeed or with pitch kept, and files at other rates are converted. \
                       No downmix, no clip gain or fades",
        }),
        json!({
            "id": "colour.management",
            "name": "Colour management",
            "group": "Colour",
            "state": "degraded",
            "detail": "The project carries an OpenColorIO config (the released ACES 2.0 CG \
                       config by default) and every media file has an input colorspace from \
                       its file rules. Displays, views and conversions match OpenColorIO 2.5.2 \
                       on the released configs (aces2_oracle.rs, released_configs.rs). The \
                       native window shows the picture through the chosen view; the browser \
                       shows decoded pictures untransformed, and neither grades",
        }),
        json!({
            "id": "delivery.render",
            "name": "Render the cut: frames or a movie with its sound",
            "group": "Delivery",
            "state": "degraded",
            "detail": "A background job writes the cut under derived/render/ as PNG frames \
                       (8 or 16-bit) or a QuickTime movie: H.264 constrained baseline and \
                       24-bit PCM stereo 48 kHz from the player's own mixer, sample-exact per \
                       picture. Retimes and angles as the monitor plays them; as decoded or \
                       through the project's view; an explicit output size and fit; captions \
                       burned in. Each render knows whether it is still current from what it \
                       depended on. Checked against the files picture by picture and sample by \
                       sample, and against GStreamer (render_movie.rs, mov_roundtrip.rs). \
                       8-bit sources only; no ProRes, DNxHR or AAC; no timecode track; CPU only",
        }),
        json!({
            "id": "colour.scopes",
            "name": "Scopes",
            "group": "Colour",
            "state": "unavailable",
            "detail": "Implemented in cine-color, not wired to any viewer",
        }),
    ];

    json!({
        "capabilities": session
            .into_iter()
            .chain(paths)
            .chain(codecs)
            .collect::<Vec<_>>()
    })
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
/// One day's call sheet as a PDF, for printing.
///
/// Binary, so it does not go through the JSON protocol: the server serves it
/// at `/callsheet?day=N` and the command line writes it to a file.
pub fn call_sheet_pdf(p: &OmniProject, day: usize) -> std::result::Result<Vec<u8>, String> {
    let doc = first_document(p).ok_or_else(|| "no screenplay imported".to_string())?;
    let g = p.engine.read();
    let schedule = EntityId::derived(doc, kinds::LOGISTICS_SCHEDULE);
    let day_id = cine_logistics::graph::day_id(schedule, day);
    let sheet = cine_logistics::graph::read_call_sheet(&g, day_id)
        .ok_or_else(|| format!("day {day} has no call sheet"))?;
    let production = g
        .of_kind(kinds::PROJECT)
        .first()
        .map(|n| n.name())
        .unwrap_or_else(|| "PRODUCTION".into());
    Ok(cine_logistics::callsheet::call_sheet_pdf(
        &sheet,
        &production,
    ))
}

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
        "edl" => {
            // The cut as CMX3600, with what the format could not carry said
            // in the file itself rather than left for the receiver to find.
            let g = p.engine.read();
            let timeline_id = g
                .of_kind(kinds::NLE_TIMELINE)
                .first()
                .map(|n| n.id)
                .ok_or_else(|| "no timeline in this project".to_string())?;
            let timeline = cine_timeline::graph::export_timeline(&g, timeline_id)
                .map_err(|e| e.to_string())?;
            let track = scene.unwrap_or(0);
            let (text, report) = cine_timeline::edl::write_edl(&timeline, track, &timeline.name);
            let mut out = text;
            for note in &report.left_behind {
                out.push_str(&format!("* NOT IN THIS EDL: {}\n", note.to_uppercase()));
            }
            out
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
        "dood" => {
            let g = p.engine.read();
            cine_logistics::graph::read_schedule(&g, doc)
                .map(|s| cine_logistics::day_out_of_days(&s).to_markdown())
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

/// An in/out pair as the wire carries it: `{"in": <time>|null, "out": ...}`.
fn marks(params: &J, which: &str) -> std::result::Result<cine_timeline::Marks, String> {
    let obj = params.get(which);
    let point = |key: &str| -> std::result::Result<Option<RationalTime>, String> {
        match obj.and_then(|o| o.get(key)) {
            None | Some(J::Null) => Ok(None),
            Some(v) => serde_json::from_value(v.clone())
                .map(Some)
                .map_err(|e| format!("invalid {which} {key} mark: {e}")),
        }
    };
    Ok(cine_timeline::Marks {
        in_point: point("in")?,
        out_point: point("out")?,
    })
}

fn plan_json(plan: &cine_timeline::Plan) -> J {
    json!({
        "source": crate::projection::range(plan.source),
        "record": crate::projection::range(plan.record),
        "backtimed": plan.backtimed,
        "retime": plan.retime.as_ref().map(|r| json!({
            "played": crate::projection::time(r.played),
            "reversed": r.reversed,
        })),
        "derived": plan.derived,
        "reconciled": plan.reconciled,
        "description": plan.describe(),
    })
}

/// A sequence's frame rate, which every caption time is counted in.
fn sequence_rate(g: &Graph, timeline: EntityId) -> std::result::Result<ucg_core::Rate, String> {
    let t = cine_timeline::graph::export_timeline(g, timeline).map_err(|e| e.to_string())?;
    t.global_start_time
        .map(|s| s.rate_exact())
        .ok_or_else(|| "the sequence declares no frame rate to count caption times in".into())
}

/// A caption track as the interface reads it: every caption with its own
/// delivery issues, the line of dialogue it transcribes and why it needs
/// review, in time order.
fn captions_json(
    g: &Graph,
    timeline: EntityId,
    track: &cine_timeline::captions::CaptionTrack,
) -> J {
    use cine_timeline::caption_link::{recorded, REVIEW};
    use cine_timeline::captions::{check, Placement, Rules};
    let rules = Rules::default();
    let issues = check(track, &rules);
    let link_of = |key: &Option<String>| -> (J, J) {
        let Some(key) = key else {
            return (J::Null, J::Null);
        };
        let id = cine_timeline::graph::caption_node(timeline, &track.language, key);
        let review = g
            .node(id)
            .and_then(|n| n.get_str(REVIEW))
            .map(|s| J::String(s.to_string()))
            .unwrap_or(J::Null);
        let link = recorded(g, id).map(|r| {
            let line = g.targets(id, EdgeKind::DerivedFrom).first().copied();
            json!({
                "line": line,
                "scene": r.scene,
                "heading": g.node(r.scene).and_then(|n| n.get_str(attrs::HEADING)),
                "character": r.character,
                "transcribed": r.text,
                "now": line.and_then(|l| g.node(l)).and_then(|n| n.get_str(attrs::TEXT)),
            })
        });
        (link.unwrap_or(J::Null), review)
    };
    json!({
        "language": track.language,
        "rate_num": track.rate.num(),
        "rate_den": track.rate.den(),
        "rules": rules,
        "captions": track.captions.iter().enumerate().map(|(i, c)| {
            let (link, review) = link_of(&c.key);
            json!({
            "index": i,
            "key": c.key,
            "link": link,
            "review": review,
            "record": crate::projection::range(c.record),
            "lines": c.lines,
            "placement": match c.placement { Placement::Bottom => "bottom", Placement::Top => "top" },
            "speaker": c.speaker,
            "issues": issues.iter().filter(|x| x.caption == i).collect::<Vec<_>>(),
        })}).collect::<Vec<_>>(),
        "issue_count": issues.len(),
    })
}

/// Read, change and write one language's captions as one undoable commit.
fn with_captions(
    p: &OmniProject,
    timeline: EntityId,
    language: &str,
    label: &str,
    change: impl FnOnce(&mut cine_timeline::captions::CaptionTrack) -> std::result::Result<(), String>,
) -> RpcResult {
    let rate = {
        let g = p.engine.read();
        sequence_rate(&g, timeline)?
    };
    let existing = {
        let g = p.engine.read();
        cine_timeline::graph::export_captions(&g, timeline, language).map_err(|e| e.to_string())?
    };
    let mut track =
        existing.unwrap_or_else(|| cine_timeline::captions::CaptionTrack::new(language, rate));
    change(&mut track)?;
    track.sort();
    let group = p
        .engine
        .transact(label, |tx| {
            cine_timeline::graph::write_captions(tx, timeline, &track)
                .map(|_| ())
                .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))
        })
        .map_err(|x| x.to_string())?;
    let mut out = with_impact(p, &group);
    // Read back: storing assigns keys, and the reactors may have flagged.
    let g = p.engine.read();
    if let Some(stored) =
        cine_timeline::graph::export_captions(&g, timeline, language).map_err(|e| e.to_string())?
    {
        out["track"] = captions_json(&g, timeline, &stored);
    }
    Ok(out)
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
        "three_point" => {
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
            let media = entity(params, "media")?;
            let source = marks(params, "source")?;
            let record = marks(params, "record")?;
            let kind = match params.get("mode").and_then(J::as_str) {
                Some("overwrite") => cine_timeline::EditKind::Overwrite,
                _ => cine_timeline::EditKind::Insert,
            };
            let on_mismatch = match params.get("mismatch").and_then(J::as_str) {
                Some("trim_tail") => cine_timeline::Mismatch::TrimSourceTail,
                Some("trim_head") => cine_timeline::Mismatch::TrimSourceHead,
                Some("honour_source") => cine_timeline::Mismatch::HonourSource,
                Some("fit_to_fill") => cine_timeline::Mismatch::FitToFill,
                _ => cine_timeline::Mismatch::Refuse,
            };
            let label = match kind {
                cine_timeline::EditKind::Overwrite => "edit: overwrite",
                cine_timeline::EditKind::Insert => "edit: insert",
            };
            let mut done = None;
            let group = p
                .engine
                .transact(label, |tx| {
                    done = Some(
                        cine_timeline::graph::three_point_edit(
                            tx,
                            &tracks,
                            media,
                            source,
                            record,
                            kind,
                            on_mismatch,
                        )
                        .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))?,
                    );
                    Ok(())
                })
                .map_err(|x| x.to_string())?;
            let (plan, placed) = done.ok_or("the edit did not run")?;
            let mut out = with_impact(p, &group);
            out["clips"] = json!(placed);
            out["plan"] = plan_json(&plan);
            out
        }
        "retime" => {
            let clip = entity(params, "clip")?;
            // A `played` of null takes the retime off and the clip plays in
            // real time again. A `ramp` — segments of `{ length, from, to }` with
            // speeds as an editor types them (`150%`, `3/2`, `1.5`) — decides the
            // length itself, so `played` is then only a placeholder.
            let ramp = match params.get("ramp") {
                None | Some(J::Null) => None,
                Some(J::Array(segments)) => Some(
                    segments
                        .iter()
                        .enumerate()
                        .map(|(i, seg)| {
                            let speed =
                                |key: &str| -> std::result::Result<ucg_core::Ratio, String> {
                                    let text = match seg.get(key) {
                                        Some(J::String(s)) => s.clone(),
                                        Some(J::Number(n)) => n.to_string(),
                                        _ => {
                                            return Err(format!(
                                                "ramp segment {} has no `{key}` speed",
                                                i + 1
                                            ))
                                        }
                                    };
                                    ucg_core::Ratio::parse(&text).ok_or_else(|| {
                                        format!("ramp segment {}: `{text}` is not a speed", i + 1)
                                    })
                                };
                            Ok(ucg_core::Ramp {
                                length: serde_json::from_value(
                                    seg.get("length").cloned().unwrap_or(J::Null),
                                )
                                .map_err(|e| {
                                    format!("ramp segment {}: invalid length: {e}", i + 1)
                                })?,
                                from: speed("from")?,
                                to: speed("to")?,
                            })
                        })
                        .collect::<std::result::Result<Vec<_>, String>>()?,
                ),
                Some(_) => return Err("`ramp` must be a list of segments".into()),
            };
            let played = match (params.get("played"), &ramp) {
                (None | Some(J::Null), None) => None,
                (Some(v), _) if !v.is_null() => Some(
                    serde_json::from_value::<RationalTime>(v.clone())
                        .map_err(|e| format!("invalid played length: {e}"))?,
                ),
                (_, Some(r)) => r.first().map(|s| s.length),
                _ => None,
            };
            let retime = played.map(|played| cine_timeline::Retime {
                played,
                reversed: params.get("reversed").and_then(J::as_bool).unwrap_or(false),
                keep_pitch: params
                    .get("keep_pitch")
                    .and_then(J::as_bool)
                    .unwrap_or(false),
                ramp: ramp.clone(),
            });
            let group = p
                .engine
                .transact(
                    if retime.is_some() {
                        "retime clip"
                    } else {
                        "clip plays in real time"
                    },
                    |tx| {
                        cine_timeline::graph::set_clip_retime(tx, clip, retime)
                            .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))
                    },
                )
                .map_err(|x| x.to_string())?;
            with_impact(p, &group)
        }
        "angles_at" => {
            // Every angle of the multicam clip on screen at `time`: which
            // picture of which file each shows, or why none.
            let timeline = entity(params, "timeline")?;
            let at: RationalTime =
                serde_json::from_value(params.get("time").cloned().ok_or("missing param `time`")?)
                    .map_err(|e| format!("invalid time: {e}"))?;
            let g = p.engine.read();
            match cine_timeline::graph::angles_at(&g, timeline, at).map_err(|e| e.to_string())? {
                None => json!({ "clip": J::Null, "angles": [] }),
                Some((clip, angles)) => json!({
                    "clip": clip,
                    "angles": angles.iter().map(|a| {
                        let available = a.asset
                            .and_then(|id| g.node(id))
                            .and_then(|n| n.get(attrs::AVAILABLE_RANGE))
                            .and_then(Value::as_range);
                        // Pictures are counted at the rate the file declares
                        // its range in, as the record monitor counts them.
                        let index = a.media.zip(available).map(|(m, r)| {
                            m.checked_sub(&r.start_time)
                                .rescale_checked(r.start_time.rate_exact(), ucg_core::Rounding::Floor)
                                .0
                                .ticks()
                        });
                        json!({
                            "key": a.key,
                            "active": a.active,
                            "asset": a.asset,
                            "name": a.name,
                            "offset": crate::projection::time(a.offset),
                            "media": a.media.map(crate::projection::time),
                            "index": index,
                            "why_not": a.why_not,
                        })
                    }).collect::<Vec<_>>(),
                }),
            }
        }
        "add_angles" => {
            let clip = entity(params, "clip")?;
            let angles: Vec<(String, EntityId)> = params
                .get("angles")
                .and_then(J::as_array)
                .ok_or("missing param `angles`")?
                .iter()
                .map(|v| {
                    let key = v
                        .get("key")
                        .and_then(J::as_str)
                        .ok_or("an angle needs a `key`")?
                        .to_string();
                    let media = v
                        .get("media")
                        .and_then(J::as_str)
                        .and_then(EntityId::parse)
                        .ok_or("an angle needs a `media` entity id")?;
                    Ok::<_, String>((key, media))
                })
                .collect::<std::result::Result<_, _>>()?;
            let group = p
                .engine
                .transact("add angles", |tx| {
                    cine_timeline::graph::add_angles(tx, clip, &angles)
                        .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))
                })
                .map_err(|x| x.to_string())?;
            with_impact(p, &group)
        }
        "set_angle" => {
            let clip = entity(params, "clip")?;
            let key = str_param(params, "key")?.to_string();
            let group = p
                .engine
                .transact(&format!("angle {key}"), |tx| {
                    cine_timeline::graph::set_clip_angle(tx, clip, &key)
                        .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))
                })
                .map_err(|x| x.to_string())?;
            with_impact(p, &group)
        }
        "switch_angle" => {
            let track = entity(params, "track")?;
            let key = str_param(params, "key")?.to_string();
            let at: RationalTime =
                serde_json::from_value(params.get("at").cloned().ok_or("missing param `at`")?)
                    .map_err(|e| format!("invalid time: {e}"))?;
            let mut switched = None;
            let group = p
                .engine
                .transact(&format!("angle {key}"), |tx| {
                    switched = Some(
                        cine_timeline::graph::switch_angle(tx, track, at, &key)
                            .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))?,
                    );
                    Ok(())
                })
                .map_err(|x| x.to_string())?;
            let mut out = with_impact(p, &group);
            out["clip"] = json!(switched.ok_or("the switch did not run")?);
            out
        }
        "captions" => {
            let timeline = entity(params, "timeline")?;
            let g = p.engine.read();
            let languages = cine_timeline::graph::caption_languages(&g, timeline);
            let language = params
                .get("language")
                .and_then(J::as_str)
                .map(str::to_string)
                .or_else(|| languages.first().cloned());
            let track = match &language {
                Some(l) => cine_timeline::graph::export_captions(&g, timeline, l)
                    .map_err(|e| e.to_string())?,
                None => None,
            };
            json!({
                "languages": languages,
                "language": language,
                "track": track.as_ref().map(|t| captions_json(&g, timeline, t)),
            })
        }
        "set_caption" => {
            let timeline = entity(params, "timeline")?;
            let language = str_param(params, "language")?.to_string();
            let time = |key: &str| -> std::result::Result<RationalTime, String> {
                serde_json::from_value(
                    params
                        .get(key)
                        .cloned()
                        .ok_or_else(|| format!("missing param `{key}`"))?,
                )
                .map_err(|e| format!("invalid `{key}`: {e}"))
            };
            let (start, end) = (time("start")?, time("end")?);
            if end <= start {
                return Err("a caption has to end after it starts".into());
            }
            let text = str_param(params, "text")?.to_string();
            let index = params.get("index").and_then(J::as_u64).map(|i| i as usize);
            let placement = match params.get("placement").and_then(J::as_str) {
                Some("top") => cine_timeline::captions::Placement::Top,
                _ => cine_timeline::captions::Placement::Bottom,
            };
            let speaker = params
                .get("speaker")
                .and_then(J::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string);
            let label = if index.is_some() {
                "edit caption"
            } else {
                "add caption"
            };
            return with_captions(p, timeline, &language, label, |track| {
                let mut caption = cine_timeline::captions::Caption::new(
                    ucg_core::TimeRange::from_start_end(start, end),
                    &text,
                );
                caption.placement = placement;
                caption.speaker = speaker;
                match index {
                    Some(i) => {
                        let slot = track
                            .captions
                            .get_mut(i)
                            .ok_or_else(|| format!("there is no caption {i}"))?;
                        // The same caption, edited: it keeps its identity, and
                        // with it the line of dialogue it is linked to.
                        caption.key = slot.key.clone();
                        *slot = caption;
                    }
                    None => track.captions.push(caption),
                }
                Ok(())
            });
        }
        "remove_caption" => {
            let timeline = entity(params, "timeline")?;
            let language = str_param(params, "language")?.to_string();
            let index = params
                .get("index")
                .and_then(J::as_u64)
                .ok_or("missing param `index`")? as usize;
            return with_captions(p, timeline, &language, "remove caption", |track| {
                if index >= track.captions.len() {
                    return Err(format!("there is no caption {index}"));
                }
                track.captions.remove(index);
                Ok(())
            });
        }
        "import_captions" => {
            let timeline = entity(params, "timeline")?;
            let language = str_param(params, "language")?.to_string();
            let text = str_param(params, "text")?.to_string();
            let rate = {
                let g = p.engine.read();
                sequence_rate(&g, timeline)?
            };
            // SCC is recognised by its header; anything else is read as SubRip.
            let (imported, report) = if text.trim_start().starts_with("Scenarist_SCC") {
                let scc =
                    cine_timeline::scc::read_scc(&text, &language).map_err(|e| e.to_string())?;
                // SCC is timed at 29.97; the sequence may not be. Every time is
                // moved onto the sequence's frames through SubRip's rounding
                // rule, which reports what moved.
                let (srt, _) = cine_timeline::captions::write_srt(&scc);
                let mut back = cine_timeline::captions::read_srt(&srt, &language, rate)
                    .map_err(|e| e.to_string())?;
                for (c, s) in back.0.captions.iter_mut().zip(&scc.captions) {
                    c.placement = s.placement;
                }
                back
            } else {
                cine_timeline::captions::read_srt(&text, &language, rate)
                    .map_err(|e| e.to_string())?
            };
            let mut out = with_captions(p, timeline, &language, "import captions", |track| {
                *track = imported;
                Ok(())
            })?;
            out["report"] = json!({
                "captions": report.captions,
                "snapped": report.snapped,
                "largest_snap_ms": report.largest_snap_ms,
                "skipped": report.skipped,
            });
            out
        }
        "caption_candidates" => {
            let timeline = entity(params, "timeline")?;
            let language = str_param(params, "language")?;
            let key = str_param(params, "key")?;
            let g = p.engine.read();
            let track = cine_timeline::graph::export_captions(&g, timeline, language)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("the sequence has no `{language}` captions"))?;
            let caption = track
                .captions
                .iter()
                .find(|c| c.key.as_deref() == Some(key))
                .ok_or_else(|| format!("there is no caption `{key}`"))?;
            let index = cine_timeline::caption_link::CutIndex::of(&g, timeline)
                .map_err(|e| e.to_string())?;
            let found = cine_timeline::caption_link::candidates(
                &g,
                &index,
                caption.record,
                &caption.lines.join(" "),
            );
            json!({
                "scenes": index.scenes_under(&g, caption.record),
                "candidates": found,
            })
        }
        "link_caption" | "unlink_caption" | "accept_caption_line" => {
            let timeline = entity(params, "timeline")?;
            let language = str_param(params, "language")?.to_string();
            let key = str_param(params, "key")?.to_string();
            let caption = cine_timeline::graph::caption_node(timeline, &language, &key);
            if !p.engine.read().contains(caption) {
                return Err(format!("there is no caption `{key}`"));
            }
            let line = if method == "link_caption" {
                Some(entity(params, "line")?)
            } else {
                None
            };
            let label = match method {
                "link_caption" => "link caption to line",
                "unlink_caption" => "unlink caption",
                _ => "accept rewritten line",
            };
            let group = p
                .engine
                .transact(label, |tx| {
                    let r = match (method, line) {
                        ("link_caption", Some(line)) => {
                            cine_timeline::caption_link::link(tx, caption, line)
                        }
                        ("unlink_caption", _) => cine_timeline::caption_link::unlink(tx, caption),
                        _ => cine_timeline::caption_link::accept(tx, caption),
                    };
                    r.map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))
                })
                .map_err(|x| x.to_string())?;
            let mut out = with_impact(p, &group);
            let g = p.engine.read();
            if let Some(track) = cine_timeline::graph::export_captions(&g, timeline, &language)
                .map_err(|e| e.to_string())?
            {
                out["track"] = captions_json(&g, timeline, &track);
            }
            out
        }
        "link_captions_exact" => {
            let timeline = entity(params, "timeline")?;
            let language = str_param(params, "language")?.to_string();
            let mut linked = 0;
            let group = p
                .engine
                .transact("link captions to their lines", |tx| {
                    linked = cine_timeline::caption_link::link_exact(tx, timeline, &language)
                        .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))?;
                    Ok(())
                })
                .map_err(|x| x.to_string())?;
            let mut out = with_impact(p, &group);
            out["linked"] = json!(linked);
            let g = p.engine.read();
            if let Some(track) = cine_timeline::graph::export_captions(&g, timeline, &language)
                .map_err(|e| e.to_string())?
            {
                out["track"] = captions_json(&g, timeline, &track);
            }
            out
        }
        "export_captions" => {
            let timeline = entity(params, "timeline")?;
            let language = str_param(params, "language")?;
            let format = str_param(params, "format")?;
            let g = p.engine.read();
            let track = cine_timeline::graph::export_captions(&g, timeline, language)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("the sequence has no `{language}` captions"))?;
            match format {
                "imsc" | "ttml" => json!({
                    "text": cine_timeline::captions::write_imsc(&track).map_err(|e| e.to_string())?,
                    "mime": "application/ttml+xml",
                    "extension": "ttml",
                }),
                "srt" => {
                    let (text, report) = cine_timeline::captions::write_srt(&track);
                    json!({ "text": text, "mime": "application/x-subrip", "extension": "srt",
                            "largest_rounding_us": report.largest_rounding_us })
                }
                "vtt" => {
                    let (text, report) = cine_timeline::captions::write_vtt(&track);
                    json!({ "text": text, "mime": "text/vtt", "extension": "vtt",
                            "largest_rounding_us": report.largest_rounding_us })
                }
                "scc" => {
                    let (text, report) =
                        cine_timeline::scc::write_scc(&track).map_err(|e| e.to_string())?;
                    json!({ "text": text, "mime": "text/x-scc", "extension": "scc",
                            "requantised": report.requantised,
                            "largest_move_ms": report.largest_move_ms,
                            "late": report.late, "latest_frames": report.latest_frames })
                }
                other => return Err(format!("no caption format `{other}` (imsc, srt, vtt, scc)")),
            }
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
        "renders" => {
            let root = p
                .directory
                .get()
                .ok_or("renders live in a project directory, and this project has none")?;
            let timeline = params
                .get("timeline")
                .and_then(J::as_str)
                .map(|t| EntityId::parse(t).ok_or("`timeline` must be an entity id"))
                .transpose()?;
            crate::provenance::renders(p, root, timeline)
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
        "script_text" => {
            let document = entity(params, "document")?;
            J::String(p.export_fountain(document).map_err(|e| e.to_string())?)
        }
        "script_write" => {
            let document = entity(params, "document")?;
            let text = str_param(params, "text")?;
            let allow = params
                .get("allow_orphans")
                .and_then(J::as_bool)
                .unwrap_or(false);
            let orphan_json = |o: &cine_pipeline::Orphan| json!({ "scene": o.scene, "heading": o.heading, "clips": o.clips, "takes": o.takes });
            match p
                .rewrite_fountain(document, text, allow)
                .map_err(|e| e.to_string())?
            {
                cine_pipeline::Rewrite::Refused(orphans) => json!({
                    "refused": true,
                    "orphans": orphans.iter().map(orphan_json).collect::<Vec<_>>(),
                }),
                cine_pipeline::Rewrite::Applied { group, orphans } => {
                    let mut out = with_impact(p, &group);
                    out["refused"] = json!(false);
                    out["orphans"] = json!(orphans.iter().map(orphan_json).collect::<Vec<_>>());
                    out
                }
            }
        }
        "production" => {
            let document = match params.get("document") {
                Some(_) => entity(params, "document")?,
                None => first_document(p).ok_or("no screenplay in this project")?,
            };
            let g = p.engine.read();
            crate::projection::production(&g, document)?
        }
        "schedule_constraints" => {
            let document = entity(params, "document")?;
            let constraints: cine_logistics::schedule::ScheduleConstraints =
                serde_json::from_value(
                    params
                        .get("constraints")
                        .cloned()
                        .ok_or("missing param `constraints`")?,
                )
                .map_err(|e| format!("invalid constraints: {e}"))?;
            // NaN and infinity are refused along with zero and negatives.
            let positive = |v: f64| v.is_finite() && v > 0.0;
            if !positive(constraints.max_pages_per_day)
                || !positive(constraints.max_hours_per_day)
                || !(1..=7).contains(&constraints.shooting_days_per_week)
            {
                return Err(
                    "constraints need positive pages and hours per day, and 1–7 shooting days a week"
                        .into(),
                );
            }
            let value = Value::from_serde(&constraints).map_err(|e| e.to_string())?;
            let group = p
                .engine
                .transact("schedule constraints", |tx| {
                    tx.set(
                        document,
                        cine_pipeline::reactors::SCHEDULE_CONSTRAINTS_ATTR,
                        value,
                    )
                })
                .map_err(|x| x.to_string())?;
            with_impact(p, &group)
        }
        "set_grade" => {
            // A CDL on a clip. The viewer bakes it into the transform it is
            // already applying, so grading and viewing are one chain rather
            // than two that can disagree.
            let clip = entity(params, "clip")?;
            let triplet = |key: &str, default: [f64; 3]| -> std::result::Result<[f64; 3], String> {
                let Some(v) = params.get(key) else {
                    return Ok(default);
                };
                let list = v
                    .as_array()
                    .ok_or(format!("`{key}` must be three numbers"))?;
                if list.len() != 3 {
                    return Err(format!("`{key}` must be three numbers"));
                }
                let mut out = [0.0; 3];
                for (i, value) in list.iter().enumerate() {
                    out[i] = value
                        .as_f64()
                        .filter(|f| f.is_finite())
                        .ok_or(format!("`{key}` must be three finite numbers"))?;
                }
                Ok(out)
            };
            let cdl = ucg_schema::Cdl {
                slope: triplet("slope", [1.0; 3])?,
                offset: triplet("offset", [0.0; 3])?,
                power: triplet("power", [1.0; 3])?,
                saturation: params
                    .get("saturation")
                    .map(|v| {
                        v.as_f64()
                            .filter(|f| f.is_finite() && *f >= 0.0)
                            .ok_or("`saturation` must be a finite number, 0 or more")
                    })
                    .transpose()?
                    .unwrap_or(1.0),
            };
            // A power of zero is not a grade, it is a flat frame.
            if cdl.power.iter().any(|p| *p <= 0.0) {
                return Err("`power` must be greater than zero on every channel".into());
            }
            {
                let g = p.engine.read();
                if !g
                    .node(clip)
                    .is_some_and(|n| n.kind.as_str() == kinds::NLE_CLIP)
                {
                    return Err(format!("{clip} is not a clip in this project"));
                }
            }
            let group = p
                .engine
                .transact("grade", |tx| {
                    cine_color::ucg::set_clip_cdl(tx, clip, &cdl)
                        .map(|_| ())
                        .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))
                })
                .map_err(|x| x.to_string())?;
            with_impact(p, &group)
        }
        "issue_revision" => {
            // Pages go out: record what the script says now, so everything
            // after it can be marked against this.
            let document = entity(params, "document")?;
            let date = params
                .get("date")
                .and_then(J::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    cine_media::mhl::iso8601_utc(std::time::SystemTime::now())
                        .split('T')
                        .next()
                        .unwrap_or_default()
                        .to_string()
                });
            let note = params
                .get("note")
                .and_then(J::as_str)
                .unwrap_or_default()
                .to_string();
            let mut issued = None;
            let group = p
                .engine
                .transact("issue revision", |tx| {
                    issued = Some(
                        cine_script::graph::issue_revision(tx, document, &date, &note)
                            .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))?,
                    );
                    Ok(())
                })
                .map_err(|x| x.to_string())?;
            let mut out = with_impact(p, &group);
            if let Some(r) = issued {
                out["revision"] = json!({
                    "colour": r.colour.name(),
                    "round": r.round,
                    "date": r.date,
                    "label": r.label(),
                    "elements": r.snapshot.text.len(),
                });
            }
            out
        }
        "tag_breakdown" => {
            // A first assistant's correction to what the script implies. It is
            // stored on the scene, not in the derived breakdown, so a rewrite
            // re-derives around it instead of losing it.
            let scene = entity(params, "scene")?;
            let category = str_param(params, "category")?.to_string();
            let value = params
                .get("value")
                .and_then(J::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            let present = params.get("present").and_then(J::as_bool);
            let is_list = matches!(category.as_str(), "prop" | "cast");
            if is_list && value.is_empty() {
                return Err(format!("tagging a {category} needs a `value`"));
            }
            {
                let g = p.engine.read();
                if !g
                    .node(scene)
                    .is_some_and(|n| n.kind.as_str() == kinds::SCRIPT_SCENE)
                {
                    return Err(format!("{scene} is not a scene in this project"));
                }
            }
            let group = p
                .engine
                .transact("tag breakdown", |tx| {
                    let mut tags: cine_logistics::SceneTags = tx
                        .graph()
                        .attr(scene, cine_logistics::ATTR_BREAKDOWN_TAGS)
                        .and_then(|v| v.to_serde().ok())
                        .unwrap_or_default();
                    match present {
                        Some(present) => tags
                            .tag(&category, &value, present)
                            .map_err(ucg_core::UcgError::Aborted)?,
                        // No `present`: hand the category back to the script.
                        None => tags.clear(&category),
                    }
                    if tags.is_empty() {
                        tx.remove_attr(scene, cine_logistics::ATTR_BREAKDOWN_TAGS)
                    } else {
                        let value = Value::from_serde(&tags)
                            .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))?;
                        tx.set(scene, cine_logistics::ATTR_BREAKDOWN_TAGS, value)
                    }
                })
                .map_err(|x| x.to_string())?;
            with_impact(p, &group)
        }
        "move_strip" => {
            // Put a scene on a day, or let the scheduler decide again
            // (`day: null`). A pin is a constraint like any other, so it is
            // stored with them, cascades like them, and undoes like them.
            let document = entity(params, "document")?;
            let scene = entity(params, "scene")?;
            let day = match params.get("day") {
                Some(J::Null) | None => None,
                Some(v) => Some(
                    v.as_u64()
                        .filter(|d| *d >= 1)
                        .ok_or("`day` must be a shooting day number (1 or more), or null")?
                        as usize,
                ),
            };
            {
                let g = p.engine.read();
                if !g
                    .node(scene)
                    .is_some_and(|n| n.kind.as_str() == kinds::SCRIPT_SCENE)
                {
                    return Err(format!("{scene} is not a scene in this project"));
                }
            }
            let group = p
                .engine
                .transact("move strip", |tx| {
                    let mut constraints =
                        cine_pipeline::reactors::read_constraints(tx.graph(), document);
                    match day {
                        Some(d) => constraints.pinned.insert(scene.to_string(), d),
                        None => constraints.pinned.remove(&scene.to_string()),
                    };
                    let value = Value::from_serde(&constraints)
                        .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))?;
                    tx.set(
                        document,
                        cine_pipeline::reactors::SCHEDULE_CONSTRAINTS_ATTR,
                        value,
                    )
                })
                .map_err(|x| x.to_string())?;
            with_impact(p, &group)
        }
        "media_preference" => {
            // With `preference` (`original`, `proxy`, `auto`), read that
            // everywhere; without, report it.
            if let Some(v) = params.get("preference") {
                let want = v
                    .as_str()
                    .and_then(cine_pipeline::session::MediaPreference::parse)
                    .ok_or("`preference` is one of original, proxy, auto")?;
                p.session.set_media(want);
            }
            json!({ "preference": p.session.media().as_str() })
        }
        "relink" => {
            // Relink one asset to one file, by hand. Content decides; a file
            // that is not this media is refused.
            let media = entity(params, "media")?;
            let path = std::path::PathBuf::from(str_param(params, "path")?);
            crate::relink::relink_to(p, media, &path)?
        }
        "check_media" => {
            // Whether each original is its content, where the project says.
            // Offline is a normal state; only real changes are committed.
            crate::relink::check(p)?
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
        "color" => {
            // The project's colour config, what is being looked through, and
            // what each media file's colorspace is and who decided it.
            let g = p.engine.read();
            crate::projection::color(&g, p.root, &p.session)?
        }
        "color_view" => {
            // With `display`/`view`, look through it everywhere; `null` returns
            // to the config's default. Without, report what is being used.
            if params.get("display").is_some() || params.get("view").is_some() {
                let want = match (params.get("display"), params.get("view")) {
                    (Some(J::Null), _) | (_, Some(J::Null)) => None,
                    (Some(d), Some(v)) => {
                        let (display, view) = (
                            d.as_str().ok_or("`display` must be a name")?.to_string(),
                            v.as_str().ok_or("`view` must be a name")?.to_string(),
                        );
                        let g = p.engine.read();
                        let config = crate::projection::color_config(&g, p.root)
                            .ok_or("the project has no colour config")?;
                        let views = config
                            .views(&display)
                            .map_err(|e| format!("unknown display `{display}`: {e}"))?;
                        if !views.iter().any(|x| x.name.eq_ignore_ascii_case(&view)) {
                            return Err(format!("`{display}` has no view `{view}`"));
                        }
                        Some(cine_pipeline::session::ViewSelection { display, view })
                    }
                    _ => return Err("give both `display` and `view`, or null".into()),
                };
                p.session.set_view(want);
            }
            let g = p.engine.read();
            crate::projection::color(&g, p.root, &p.session)?
        }
        "set_input_colorspace" => {
            let media = entity(params, "media")?;
            let name = params
                .get("colorspace")
                .and_then(J::as_str)
                .ok_or("missing param `colorspace`")?
                .to_string();
            let group = p
                .engine
                .transact("input colorspace", |tx| {
                    let config =
                        crate::projection::color_config(tx.graph(), p.root).ok_or_else(|| {
                            ucg_core::UcgError::Aborted("the project has no colour config".into())
                        })?;
                    cine_color::ucg::set_input_colorspace(tx, media, &config, &name)
                        .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))
                })
                .map_err(|e| e.to_string())?;
            with_impact(p, &group)
        }
        "edit_list" => {
            let timeline = entity(params, "timeline")?;
            let g = p.engine.read();
            crate::projection::edit_list(&g, timeline, p.session.media())?
        }
        "picture_at" => {
            let timeline = entity(params, "timeline")?;
            let at: RationalTime =
                serde_json::from_value(params.get("time").cloned().ok_or("missing param `time`")?)
                    .map_err(|e| format!("invalid time: {e}"))?;
            let g = p.engine.read();
            crate::projection::picture_at(&g, timeline, at, p.session.media())?
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
