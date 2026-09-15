//! Projections of the project graph, for clients that should not re-derive it.
//!
//! ## Why a projection and not raw nodes
//!
//! A timeline is a tree of stacks, tracks, clips and time ranges whose visible
//! shape comes from *composition* — where each item starts is the sum of what
//! precedes it, at exact rational rates that need not match. A client walking
//! `node` calls would need dozens of round trips and would end up implementing
//! that arithmetic itself, in a second language, against a second notion of
//! time. That is precisely the second source of truth this project exists
//! without.
//!
//! So the runtime computes the shape and sends the answer. The client draws it
//! and sends *intents* back. Times are carried both exactly (ticks and rate) and
//! as seconds: the exact pair is authoritative, and the seconds are for layout,
//! which is the one place a float is harmless.

use cine_timeline::graph as tl;
use cine_timeline::session::TrackState;
use serde_json::{json, Value as J};
use ucg_core::{EntityId, Graph, RationalTime, TimeRange};
use ucg_schema::{attrs, kinds};

/// An exact time, plus a convenience value for drawing.
///
/// `ticks`/`rate` is the number; `seconds` is a rendering aid and must never be
/// what a decision is made from.
pub fn time(t: RationalTime) -> J {
    json!({
        "ticks": t.ticks(),
        "rate_num": t.rate_exact().num(),
        "rate_den": t.rate_exact().den(),
        "seconds": t.to_seconds(),
        "timecode": t.to_timecode(),
    })
}

pub fn range(r: TimeRange) -> J {
    json!({
        "start": time(r.start_time),
        "duration": time(r.duration),
        "end": time(r.end_time_exclusive()),
    })
}

fn name_of(graph: &Graph, id: EntityId) -> Option<String> {
    graph.node(id).map(|n| n.name())
}

/// Project identity and session facts.
pub fn project(
    graph: &Graph,
    stats: &ucg_core::EngineStats,
    root: &std::path::Path,
    clients: usize,
    uptime_s: u64,
) -> J {
    let project_node = graph.of_kind(kinds::PROJECT).first().map(|n| n.id);
    json!({
        "name": project_node
            .and_then(|id| name_of(graph, id))
            .unwrap_or_else(|| "untitled".into()),
        "id": project_node,
        "path": root.display().to_string(),
        "revision": stats.revision,
        "nodes": stats.nodes,
        "edges": stats.edges,
        "undo_depth": stats.undo_depth,
        "by_kind": stats.by_kind,
        "log_len": stats.log_len,
        "clients": clients,
        "uptime_s": uptime_s,
    })
}

/// Every timeline in the project, shallowly.
pub fn timelines(graph: &Graph) -> J {
    J::Array(
        graph
            .of_kind(kinds::NLE_TIMELINE)
            .iter()
            .map(|n| {
                let tracks = tl::export_timeline(graph, n.id)
                    .map(|t| t.tracks().count())
                    .unwrap_or(0);
                json!({ "id": n.id, "name": n.name(), "tracks": tracks })
            })
            .collect(),
    )
}

/// One timeline, composed: every track, every item, every item's place on the
/// track, and the editorial state the interface needs to draw the headers.
pub fn timeline(graph: &Graph, id: EntityId) -> Result<J, String> {
    let composed = tl::export_timeline(graph, id).map_err(|e| e.to_string())?;
    // Track *node* ids, in the same order as the exported tracks, so the client
    // can address a track by entity rather than by position.
    let track_ids = track_entity_ids(graph, id);

    let mut tracks = Vec::new();
    for (index, track) in composed.tracks().enumerate() {
        let state = TrackState::of(track);
        let mut items = Vec::new();
        for (i, item) in track.children.iter().enumerate() {
            let Some(at) = track.range_of_child(i) else {
                continue;
            };
            let clip = item.as_clip();
            items.push(json!({
                "index": i,
                "id": item.ucg_id(),
                "kind": match item {
                    cine_timeline::Item::Clip(_) => "clip",
                    cine_timeline::Item::Gap(_) => "gap",
                    cine_timeline::Item::Transition(_) => "transition",
                },
                "name": item.name(),
                "at": range(at),
                "source_range": item.source_range().map(range),
                "enabled": clip.map(|c| c.enabled).unwrap_or(true),
                "media": clip.map(|c| c.media_reference.target_url()),
                "available_range": clip.and_then(|c| c.available_range()).map(range),
                "handles": clip
                    .and_then(cine_timeline::session::handles)
                    .map(|(head, tail)| json!({ "head": time(head), "tail": time(tail) })),
                "link": clip.and_then(cine_timeline::session::link_of),
                "needs_conform": item.ucg_id().and_then(|c| graph.node(c))
                    .and_then(|n| n.get_bool(attrs::NEEDS_CONFORM)).unwrap_or(false),
                "conform_reason": item.ucg_id().and_then(|c| graph.node(c))
                    .and_then(|n| n.get_str(attrs::DIRTY_REASON)).map(str::to_string),
                "scenes": item.ucg_id().filter(|_| clip.is_some())
                    .map(|c| tl::scenes_of_clip(graph, c)).unwrap_or_default(),
                "asset": item.ucg_id().filter(|_| clip.is_some())
                    .and_then(|c| tl::media_of_clip(graph, c)),
            }));
        }
        tracks.push(json!({
            "index": index,
            "id": track_ids.get(index),
            "name": track.name,
            "kind": match track.kind {
                cine_timeline::TrackKind::Video => "video",
                cine_timeline::TrackKind::Audio => "audio",
            },
            "enabled": track.enabled,
            "locked": state.locked,
            "muted": state.muted,
            "solo": state.solo,
            "targeted": state.targeted,
            "duration": time(track.duration()),
            "items": items,
            "markers": track.markers.iter().map(|m| json!({
                "name": m.name,
                "at": range(m.marked_range),
                "color": m.color,
            })).collect::<Vec<_>>(),
        }));
    }

    let duration = composed
        .tracks()
        .map(|t| t.duration())
        .max()
        .unwrap_or_else(|| RationalTime::exact(0, ucg_core::Rate::F24));

    Ok(json!({
        "id": id,
        "name": composed.name,
        "duration": time(duration),
        "global_start_time": composed.global_start_time.map(time),
        "tracks": tracks,
        "any_solo_video": cine_timeline::session::any_solo_of(
            &composed, cine_timeline::TrackKind::Video),
        "any_solo_audio": cine_timeline::session::any_solo_of(
            &composed, cine_timeline::TrackKind::Audio),
    }))
}

/// Track node ids under a timeline, in composition order.
fn track_entity_ids(graph: &Graph, timeline: EntityId) -> Vec<EntityId> {
    let mut out = Vec::new();
    for stack in graph.children(timeline) {
        for child in graph.children(stack) {
            if graph
                .node(child)
                .map(|n| n.kind.as_str() == kinds::NLE_TRACK)
                .unwrap_or(false)
            {
                out.push(child);
            }
        }
    }
    out
}

/// Media registered in the project, with what the file actually declares.
pub fn media(graph: &Graph) -> J {
    J::Array(
        graph
            .of_kind(kinds::ASSET_MEDIA)
            .iter()
            .map(|n| {
                let info = cine_media::ucg::read_media(graph, n.id);
                let streams: Vec<J> = graph
                    .children(n.id)
                    .into_iter()
                    .filter_map(|s| {
                        let sn = graph.node(s)?;
                        Some(json!({
                            "id": s,
                            "name": sn.name(),
                            "codec": sn.get_str(cine_media::ucg::CODEC_ATTR),
                            "decodable": sn.get_bool(cine_media::ucg::DECODABLE_ATTR),
                        }))
                    })
                    .collect();
                json!({
                    "id": n.id,
                    "name": n.name(),
                    "path": n.get_str(attrs::MEDIA_PATH),
                    "online": n.get_bool(cine_media::ucg::ONLINE_ATTR),
                    "hash": n.get_str(cine_media::ucg::HASH_ATTR),
                    "container": info.as_ref().map(|i| format!("{:?}", i.container)),
                    "size": info.as_ref().map(|i| i.hash.size()),
                    // Duration from the video stream's own time base, exactly,
                    // rather than from a seconds field the container may not have.
                    "duration_s": info
                        .as_ref()
                        .and_then(|i| i.first_video().cloned())
                        .and_then(|s| s.duration_ticks.map(|t| {
                            RationalTime::exact(t, s.time_base).to_seconds()
                        })),
                    "video": info.as_ref().and_then(|i| i.first_video().cloned()).map(|s| json!({
                        "codec": s.codec,
                        "width": s.video.map(|v| v.width),
                        "height": s.video.map(|v| v.height),
                        "frames": s.video.and_then(|v| v.frame_count),
                        "rate_num": s.video.and_then(|v| v.frame_rate).map(|r| r.num()),
                        "rate_den": s.video.and_then(|v| v.frame_rate).map(|r| r.den()),
                        "decodable": s.decodable,
                    })),
                    "streams": streams,
                })
            })
            .collect(),
    )
}

/// The production chain, as edges rather than as a guess.
///
/// screenplay scene → shots → takes → media. Each link is read from the graph;
/// nothing here infers a relationship from a name.
pub fn structure(graph: &Graph) -> J {
    let scenes: Vec<J> = graph
        .of_kind(kinds::SCRIPT_SCENE)
        .iter()
        .map(|scene| {
            let shots: Vec<J> = graph
                .of_kind(kinds::LOGISTICS_SHOT)
                .iter()
                .filter(|shot| derives_from(graph, shot.id, scene.id))
                .map(|shot| {
                    let takes: Vec<J> = graph
                        .of_kind(kinds::LOGISTICS_TAKE)
                        .iter()
                        .filter(|take| derives_from(graph, take.id, shot.id))
                        .map(|take| {
                            let bound: Vec<EntityId> =
                                graph.targets(take.id, ucg_core::EdgeKind::Binds);
                            json!({
                                "id": take.id,
                                "name": take.name(),
                                "media": bound,
                            })
                        })
                        .collect();
                    json!({ "id": shot.id, "name": shot.name(), "takes": takes })
                })
                .collect();
            json!({
                "id": scene.id,
                "heading": scene.get_str(attrs::HEADING),
                "number": scene.get_str(attrs::SCENE_NUMBER),
                "eighths": scene.get_int(attrs::PAGE_EIGHTHS),
                "est_duration_s": scene.get_float(attrs::EST_DURATION_S),
                "shots": shots,
            })
        })
        .collect();
    json!({ "scenes": scenes })
}

/// The picture a sequence shows at `at`: the top-most visible picture track
/// with a clip there, the asset that clip plays, and which picture of it.
///
/// This is composition, so it is answered here and not in the browser. A gap
/// answers `black`; a clip whose asset declares no available range answers with
/// no index, because which picture it plays cannot be known.
pub fn picture_at(graph: &Graph, timeline: EntityId, at: RationalTime) -> Result<J, String> {
    use ucg_core::Rounding;
    let composed = tl::export_timeline(graph, timeline).map_err(|e| e.to_string())?;
    let solo = cine_timeline::session::any_solo_of(&composed, cine_timeline::TrackKind::Video);
    let track_ids = track_entity_ids(graph, timeline);
    let tracks: Vec<(usize, &cine_timeline::Track)> = composed.tracks().enumerate().collect();
    for (ti, track) in tracks.into_iter().rev() {
        if track.kind != cine_timeline::TrackKind::Video
            || !track.enabled
            || !cine_timeline::session::is_visible(track, solo)
        {
            continue;
        }
        for (i, item) in track.children.iter().enumerate() {
            let Some(clip) = item.as_clip() else { continue };
            if !clip.enabled {
                continue;
            }
            let Some(span) = track.range_of_child(i) else {
                continue;
            };
            if !span.contains(at) {
                continue;
            }
            let Some(source) = clip.trimmed_range() else {
                continue;
            };
            let offset = at.checked_sub(&span.start_time);
            let source_time = source.start_time.checked_add(&offset);
            let asset = item.ucg_id().and_then(|c| tl::media_of_clip(graph, c));
            let available = asset
                .and_then(|a| graph.node(a))
                .and_then(|n| n.get(attrs::AVAILABLE_RANGE))
                .and_then(ucg_core::Value::as_range);
            let index = available.map(|a| {
                let rate = a.start_time.rate_exact();
                let (t, _) = source_time.rescale_checked(rate, Rounding::Floor);
                t.ticks() - a.start_time.ticks()
            });
            return Ok(json!({
                "black": false,
                "track": track_ids.get(ti),
                "track_name": track.name,
                "clip": item.ucg_id(),
                "name": item.name(),
                "asset": asset,
                "online": asset.and_then(|a| graph.node(a))
                    .and_then(|n| n.get_bool(cine_media::ucg::ONLINE_ATTR)),
                "source": time(source_time),
                "index": index.filter(|i| *i >= 0),
            }));
        }
    }
    Ok(json!({ "black": true }))
}

/// Children of `parent` of `kind`, in their stored order.
fn ordered_children(graph: &Graph, parent: EntityId, kind: Option<&str>) -> Vec<EntityId> {
    let mut out: Vec<(i64, EntityId)> = graph
        .children(parent)
        .into_iter()
        .filter_map(|c| {
            let n = graph.node(c)?;
            if kind.is_some_and(|k| n.kind.as_str() != k) {
                return None;
            }
            Some((n.get_int(attrs::ORDER).unwrap_or(i64::MAX), c))
        })
        .collect();
    out.sort();
    out.into_iter().map(|(_, c)| c).collect()
}

/// The screenplay as the script workspace reads it: every document, its scenes
/// in order, each scene's elements, and the cut material bound to the scene.
pub fn script(graph: &Graph) -> J {
    let documents: Vec<J> = graph
        .of_kind(kinds::SCRIPT_DOCUMENT)
        .iter()
        .map(|doc| {
            let scenes: Vec<J> = ordered_children(graph, doc.id, Some(kinds::SCRIPT_SCENE))
                .into_iter()
                .filter_map(|scene| {
                    let n = graph.node(scene)?;
                    let elements: Vec<J> = ordered_children(graph, scene, None)
                        .into_iter()
                        .filter_map(|e| {
                            let en = graph.node(e)?;
                            let kind = en.kind.as_str().strip_prefix("script.")?;
                            Some(json!({
                                "id": e,
                                "kind": kind,
                                "text": en.get_str(attrs::TEXT),
                                "character": en.get_str(attrs::CHARACTER),
                            }))
                        })
                        .collect();
                    let clips: Vec<J> = tl::clips_synced_to(graph, scene)
                        .into_iter()
                        .filter_map(|c| {
                            let cn = graph.node(c)?;
                            Some(json!({
                                "id": c,
                                "name": cn.name(),
                                "timeline": tl::timeline_of(graph, c),
                                "track": tl::track_of_item(graph, c),
                                "needs_conform": cn.get_bool(attrs::NEEDS_CONFORM).unwrap_or(false),
                                "reason": cn.get_str(attrs::DIRTY_REASON),
                            }))
                        })
                        .collect();
                    Some(json!({
                        "id": scene,
                        "heading": n.get_str(attrs::HEADING),
                        "number": n.get_str(attrs::SCENE_NUMBER),
                        "words": n.get_int(attrs::WORDS),
                        "eighths": n.get_int(attrs::PAGE_EIGHTHS),
                        "est_duration_s": n.get_float(attrs::EST_DURATION_S),
                        "characters": n.get(attrs::CHARACTERS).map(|v| v.to_json()),
                        "elements": elements,
                        "clips": clips,
                    }))
                })
                .collect();
            json!({
                "id": doc.id,
                "name": doc.name(),
                "source_key": doc.get_str(attrs::SOURCE_KEY),
                "scenes": scenes,
            })
        })
        .collect();
    json!({ "documents": documents })
}

/// Whether `node` is derived from or contained by `ancestor`, directly.
fn derives_from(graph: &Graph, node: EntityId, ancestor: EntityId) -> bool {
    use ucg_core::EdgeKind;
    [
        EdgeKind::DerivedFrom,
        EdgeKind::Contains,
        EdgeKind::References,
    ]
    .iter()
    .any(|k| graph.targets(node, *k).contains(&ancestor))
        || graph.targets(ancestor, EdgeKind::Contains).contains(&node)
}
