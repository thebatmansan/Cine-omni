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

/// A clip's speed: the exact ratio, and a label for a badge.
///
/// The label is display only — `2x`, `-1x`, `0.3333x` — and the ratio beside
/// it is the number. Nothing computes from the label.
fn speed_json(((media, track), reversed): ((i64, i64), bool)) -> J {
    let n = format!("{:.4}", media as f64 / track as f64);
    let n = n.trim_end_matches('0').trim_end_matches('.');
    json!({
        "media": media,
        "track": track,
        "reversed": reversed,
        "label": format!("{}{n}x", if reversed { "-" } else { "" }),
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
                "speed": clip.and_then(|c| c.speed()).map(speed_json),
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
                    "checksum": n.get(crate::jobs::CHECKSUM_ATTR).map(|v| v.to_json()),
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

/// The picture a sequence shows at `at`: the clip there on the flattened edit
/// list, the asset it plays, and which picture of it.
///
/// Composition is answered by [`crate::playlist`], the same list the native
/// player plays. A gap answers `black`; a clip whose asset declares no available
/// range answers with no index, because which picture it plays cannot be known.
pub fn picture_at(graph: &Graph, timeline: EntityId, at: RationalTime) -> Result<J, String> {
    use ucg_core::Rounding;
    let list = crate::playlist::edit_list(graph, timeline)?;
    let Some(entry) = list.at(at) else {
        return Ok(json!({ "black": true }));
    };
    let available = entry
        .asset
        .and_then(|a| graph.node(a))
        .and_then(|n| n.get(attrs::AVAILABLE_RANGE))
        .and_then(ucg_core::Value::as_range);
    // How far into the entry this instant is, carried into media time through
    // the retime when there is one. The speed's own arithmetic is exact and
    // floors onto the asset's frame grid, so this never invents a picture
    // between two pictures.
    let into = at.checked_sub(&entry.record.start_time);
    let offset = entry.offset.map(|o| {
        let media = match (entry.speed, available) {
            (Some(s), Some(a)) => s.media_offset(into, a.start_time.rate_exact()),
            _ => into,
        };
        o.checked_add(&media)
    });
    let index = offset.zip(available).map(|(o, a)| {
        // Pictures are counted at the rate the asset declares its range in.
        let (t, _) = o.rescale_checked(a.start_time.rate_exact(), Rounding::Floor);
        t.ticks()
    });
    let track_ids = track_entity_ids(graph, timeline);
    Ok(json!({
        "black": false,
        "track": track_ids.get(entry.track),
        "track_name": tl::export_timeline(graph, timeline).ok()
            .and_then(|t| t.tracks().nth(entry.track).map(|t| t.name.clone())),
        "clip": entry.clip,
        "name": entry.name,
        "asset": entry.asset,
        "online": entry.online,
        "speed": entry.speed.map(|s| json!({
            "media": s.media, "track": s.track, "reversed": s.reversed,
        })),
        "playable": entry.unplayable().is_none(),
        "why_not": entry.unplayable(),
        "source": offset.zip(available).map(|(o, a)| time(a.start_time.checked_add(&o))),
        "index": index.filter(|i| *i >= 0),
    }))
}

/// The flattened edit list, for players and for inspection.
pub fn edit_list(graph: &Graph, timeline: EntityId) -> Result<J, String> {
    let list = crate::playlist::edit_list(graph, timeline)?;
    Ok(json!({
        "timeline": list.timeline,
        "name": list.name,
        "rate_num": list.rate.num(),
        "rate_den": list.rate.den(),
        "end": time(list.end),
        "transitions_played_as_cuts": list.transitions,
        "entries": list.entries.iter().map(|e| json!({
            "record": range(e.record),
            "clip": e.clip,
            "name": e.name,
            "asset": e.asset,
            "offset": e.offset.map(time),
            "speed": e.speed.map(|s| json!({
                "media": s.media, "track": s.track, "reversed": s.reversed,
            })),
            "playable": e.unplayable().is_none(),
            "why_not": e.unplayable(),
        })).collect::<Vec<_>>(),
    }))
}

/// The production office's view of a screenplay: every scene's breakdown, the
/// stripboard the scheduler built, each day's call sheet, the constraints it
/// was built under, and the shot list with the takes shot against each shot.
///
/// All of it is derived state the logistics reactor keeps current; nothing is
/// computed here beyond gathering it.
pub fn production(graph: &Graph, document: EntityId) -> Result<J, String> {
    use cine_logistics::graph as lg;
    if !graph
        .node(document)
        .is_some_and(|n| n.kind.as_str() == kinds::SCRIPT_DOCUMENT)
    {
        return Err(format!("{document} is not a screenplay"));
    }
    let breakdowns = cine_logistics::breakdown::breakdown_document(graph, document);
    let schedule = lg::read_schedule(graph, document);
    let constraints = cine_pipeline::reactors::read_constraints(graph, document);
    let sheets: Vec<J> = schedule
        .as_ref()
        .map(|s| {
            s.days
                .iter()
                .filter_map(|d| {
                    let day = lg::day_id(lg::schedule_id(document), d.index);
                    lg::read_call_sheet(graph, day).and_then(|c| serde_json::to_value(c).ok())
                })
                .collect()
        })
        .unwrap_or_default();
    let shots: Vec<J> = breakdowns
        .iter()
        .map(|b| {
            let list: Vec<J> = lg::read_shot_list(graph, b.scene)
                .iter()
                .map(|shot| {
                    let id = lg::shot_id(shot.camera);
                    let takes: Vec<J> = lg::takes_of_shot(graph, id)
                        .into_iter()
                        .filter_map(|t| {
                            let n = graph.node(t)?;
                            Some(json!({ "id": t, "name": n.name() }))
                        })
                        .collect();
                    json!({
                        "id": id,
                        "number": shot.spec.number,
                        "size": format!("{:?}", shot.spec.size),
                        "movement": format!("{:?}", shot.spec.movement),
                        "lens_mm": shot.gear.lens_mm,
                        "camera_body": shot.gear.camera_body,
                        "support": format!("{:?}", shot.gear.support),
                        "description": shot.spec.description,
                        "stale": graph.node(id).and_then(|n| n.get_bool(attrs::STALE)).unwrap_or(false),
                        "takes": takes,
                    })
                })
                .collect();
            json!({ "scene": b.scene, "shots": list })
        })
        .collect();
    Ok(json!({
        "document": document,
        "breakdowns": breakdowns,
        "schedule": schedule.as_ref().map(|s| json!({
            "days": s.days,
            "warnings": s.warnings,
            "company_moves": s.company_moves(),
        })),
        "call_sheets": sheets,
        "constraints": constraints,
        "shots": shots,
        // Day Out of Days: what the stripboard means for the cast, and what
        // its shape costs in hold days.
        "dood": schedule.as_ref().map(|s| {
            let d = cine_logistics::day_out_of_days(s);
            json!({
                "days": d.days,
                "cast": d.cast.iter().map(|row| json!({
                    "name": row.name,
                    "days": row.days.iter().map(|s| s.code()).collect::<Vec<_>>(),
                    "start": row.start,
                    "finish": row.finish,
                    "worked": row.worked,
                    "held": row.held,
                })).collect::<Vec<_>>(),
                "total_worked": d.total_worked(),
                "total_held": d.total_held(),
            })
        }),
    }))
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
            // Revision marks: what has changed since the pages went out. The
            // marks are the difference from the newest revision's snapshot,
            // computed here rather than stamped on the elements, so an edit
            // and its undo both leave the right thing behind.
            let revisions = cine_script::graph::revisions(graph, doc.id);
            let now = cine_script::graph::snapshot(graph, doc.id);
            let marks = revisions
                .last()
                .map(|r| cine_script::revision::marks(&now, r))
                .unwrap_or_default();
            let removed = revisions
                .last()
                .map(|r| cine_script::revision::removed_since(&now, r))
                .unwrap_or_default();
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
                                // "changed" or "added" since the last revision.
                                "revised": marks.get(&e.to_string()).map(|m| match m {
                                    cine_script::revision::Mark::Changed => "changed",
                                    cine_script::revision::Mark::Added => "added",
                                }),
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
                        "revised": marks.get(&scene.to_string()).map(|m| match m {
                            cine_script::revision::Mark::Changed => "changed",
                            cine_script::revision::Mark::Added => "added",
                        }),
                    }))
                })
                .collect();
            let next_colour = {
                let (colour, round) = cine_script::revision::Colour::next(
                    revisions.last().map(|r| (r.colour, r.round)),
                );
                colour.spoken(round)
            };
            json!({
                "id": doc.id,
                "name": doc.name(),
                "source_key": doc.get_str(attrs::SOURCE_KEY),
                "scenes": scenes,
                // The pages that went out, and what has moved since.
                "revisions": revisions.iter().map(|r| json!({
                    "colour": r.colour.name(),
                    "round": r.round,
                    "date": r.date,
                    "note": r.note,
                    "label": r.label(),
                })).collect::<Vec<_>>(),
                "current_revision": revisions.last().map(|r| r.label()),
                "next_colour": next_colour,
                "marked": marks.len(),
                "removed_since_revision": removed,
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

/// The project's colour config, parsed.
pub fn color_config(graph: &Graph, root: EntityId) -> Option<cine_color::ocio::Config> {
    let id = cine_color::ucg::project_config(graph, root)?;
    let yaml = graph.node(id)?.get_str(cine_color::ucg::ATTR_YAML)?;
    cine_color::ocio::Config::parse(yaml).ok()
}

/// Colour as the application can report it: the config, the displays and views
/// it offers, what is being looked through, and every media file's input
/// colorspace with who decided it.
///
/// Nothing here is invented: a media file with no colorspace says so rather
/// than being reported as the working space, and the transform quoted for a
/// clip is the one the window applies.
pub fn color(
    graph: &Graph,
    root: EntityId,
    session: &cine_pipeline::session::ProjectSession,
) -> std::result::Result<J, String> {
    let Some(config_id) = cine_color::ucg::project_config(graph, root) else {
        return Ok(json!({ "config": J::Null, "displays": [], "media": [] }));
    };
    let config = color_config(graph, root).ok_or("the project's colour config does not parse")?;
    let node = graph.node(config_id).ok_or("no config")?;
    let displays: Vec<J> = config
        .displays
        .iter()
        .map(|(name, views)| {
            json!({
                "name": name,
                "active": config.active_displays.is_empty()
                    || config.active_displays.iter().any(|d| d == name),
                "views": views.iter().map(|v| json!({
                    "name": v.name,
                    "active": config.active_views.is_empty()
                        || config.active_views.iter().any(|a| a.eq_ignore_ascii_case(&v.name)),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let colorspaces: Vec<J> = config
        .colorspaces
        .iter()
        .map(|c| {
            json!({
                "name": c.name,
                "family": c.family,
                "encoding": c.encoding,
                "is_display": c.is_display,
                "isdata": c.isdata,
            })
        })
        .collect();
    let media: Vec<J> = graph
        .of_kind(kinds::ASSET_MEDIA)
        .iter()
        .map(|n| {
            let meta = <ucg_schema::ColorMeta as ucg_schema::Component>::read_from(graph, n.id);
            json!({
                "id": n.id,
                "name": n.name(),
                "path": n.get_str(attrs::MEDIA_PATH),
                "input_colorspace": meta.as_ref().map(|m| m.input_colorspace.clone()),
                "source": n.get_str(cine_color::ucg::ATTR_INPUT_SOURCE),
            })
        })
        .collect();
    // The clips of the open sequence, with what each is in and how it is
    // graded: what a colour workspace needs to show and to change.
    let clips: Vec<J> = session
        .focus()
        .or_else(|| {
            graph
                .of_kind(kinds::NLE_TIMELINE)
                .first()
                .map(|n| n.id)
        })
        .and_then(|timeline| crate::playlist::edit_list(graph, timeline).ok())
        .map(|list| {
            list.entries
                .iter()
                .filter_map(|entry| {
                    let clip = entry.clip?;
                    let meta = <ucg_schema::ColorMeta as ucg_schema::Component>::read_from(
                        graph,
                        EntityId::derived(clip, kinds::COLOR_GRADE),
                    );
                    let cdl = meta.as_ref().map(|m| &m.cdl);
                    Some(json!({
                        "clip": clip,
                        "name": entry.name,
                        "input_colorspace": meta.as_ref().map(|m| m.input_colorspace.clone()),
                        "graded": cdl.map(|c| !cine_color::grade::cdl_is_identity(c)).unwrap_or(false),
                        "cdl": cdl.map(|c| json!({
                            "slope": c.slope,
                            "offset": c.offset,
                            "power": c.power,
                            "saturation": c.saturation,
                        })),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    let selected = session.view();
    Ok(json!({
        "config": {
            "name": node.name(),
            "source": node.get_str(cine_color::ucg::ATTR_CONFIG_SOURCE),
            "unimplemented": config.unimplemented,
        },
        "view": selected.as_ref().map(|v| json!({ "display": v.display, "view": v.view })),
        "displays": displays,
        "colorspaces": colorspaces,
        "media": media,
        "clips": clips,
    }))
}
