//! Reading and writing logistics entities in the Unified Context Graph.
//!
//! Every writer is idempotent: derived ids are stable, `tx.set` no-ops on
//! unchanged values and `tx.link` on existing edges, so re-running a writer
//! with the same input commits nothing. Writers take `&mut Tx`, readers take
//! `&Graph`.
//!
//! Entity layout:
//!
//! ```text
//! script.scene ─┬─ DerivedFrom ← logistics.breakdown   (attr "breakdown")
//!               └─ DerivedFrom ← logistics.shot_list ─Contains→ logistics.shot ─DependsOn→ previs.camera
//!                                                                          └─Contains→ logistics.take
//! script.document ─ DerivedFrom ← logistics.schedule ─Contains→ logistics.day ─DependsOn→ script.scene
//!                                                                       └─ DerivedFrom ← logistics.call_sheet
//! ```

use crate::breakdown::SceneBreakdown;
use crate::callsheet::CallSheet;
use crate::date::Date;
use crate::error::{Error, Result};
use crate::metadata::{MetadataEvent, TakeMetadataSummary};
use crate::schedule::{Schedule, ShootDay, Strip};
use crate::shotlist::{previs_scene_of, ShotEntry};
use std::collections::HashSet;
use ucg_core::{EdgeKind, EntityId, Graph, Tx, Value};
use ucg_schema::{attrs, kinds, Component};

/// Attribute holding the serialized [`SceneBreakdown`] on a breakdown node.
pub const BREAKDOWN_ATTR: &str = "breakdown";
/// Attribute holding the serialized gear list on a shot node.
pub const GEAR_ATTR: &str = "gear";
/// Attribute holding the serialized strips on a day node.
pub const STRIPS_ATTR: &str = "strips";
/// Attribute holding the serialized [`CallSheet`] on a call-sheet node.
pub const CALL_SHEET_ATTR: &str = "call_sheet";
/// Attribute holding the serialized metadata events on a take node.
pub const EVENTS_ATTR: &str = "events";
/// Module-private attributes on schedule / day nodes.
pub const DATE_ATTR: &str = "date";
pub const TOTAL_EIGHTHS_ATTR: &str = "total_eighths";
pub const IS_NIGHT_ATTR: &str = "is_night";
pub const CAST_ON_CALL_ATTR: &str = "cast_on_call";
pub const LOCATIONS_ATTR: &str = "locations";
pub const WARNINGS_ATTR: &str = "warnings";
pub const TAKE_ATTR: &str = "take";

/// Id of the breakdown node for a scene.
pub fn breakdown_id(scene: EntityId) -> EntityId {
    EntityId::derived(scene, kinds::LOGISTICS_BREAKDOWN)
}
/// Id of the shot-list node for a scene.
pub fn shot_list_id(scene: EntityId) -> EntityId {
    EntityId::derived(scene, kinds::LOGISTICS_SHOT_LIST)
}
/// Id of the shot node for a previs camera.
pub fn shot_id(camera: EntityId) -> EntityId {
    EntityId::derived(camera, kinds::LOGISTICS_SHOT)
}
/// Id of the schedule node for a document.
pub fn schedule_id(document: EntityId) -> EntityId {
    EntityId::derived(document, kinds::LOGISTICS_SCHEDULE)
}
/// Id of day `index` (1-based) under a schedule node.
pub fn day_id(schedule: EntityId, index: usize) -> EntityId {
    EntityId::derived(schedule, &format!("day:{index}"))
}
/// Id of the call sheet for a day node.
pub fn call_sheet_id(day: EntityId) -> EntityId {
    EntityId::derived(day, kinds::LOGISTICS_CALL_SHEET)
}
/// Id of take `n` under a shot node.
pub fn take_id(shot: EntityId, take_number: u32) -> EntityId {
    EntityId::derived(shot, &format!("take:{take_number}"))
}

fn require(graph: &Graph, id: EntityId) -> Result<()> {
    if graph.contains(id) {
        Ok(())
    } else {
        Err(Error::NotFound(id))
    }
}

fn children_of_kind(graph: &Graph, parent: EntityId, kind: &str) -> Vec<EntityId> {
    graph
        .children(parent)
        .into_iter()
        .filter(|id| {
            graph
                .node(*id)
                .map(|n| n.kind.as_str() == kind)
                .unwrap_or(false)
        })
        .collect()
}

fn str_list(graph: &Graph, id: EntityId, key: &str) -> Vec<String> {
    graph
        .attr(id, key)
        .and_then(Value::as_list)
        .map(|l| {
            l.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The `script.document` containing a scene.
pub fn document_of_scene(graph: &Graph, scene: EntityId) -> Option<EntityId> {
    graph.ancestor_of_kind(scene, kinds::SCRIPT_DOCUMENT)
}

/// Store a breakdown sheet as `logistics.breakdown` derived from `scene`.
pub fn write_breakdown(tx: &mut Tx, scene: EntityId, bd: &SceneBreakdown) -> Result<EntityId> {
    require(tx.graph(), scene)?;
    let id = breakdown_id(scene);
    tx.ensure(id, kinds::LOGISTICS_BREAKDOWN)?;
    tx.set(id, attrs::NAME, format!("Breakdown {}", bd.number))?;
    tx.set(id, attrs::SCENE_NUMBER, bd.number.clone())?;
    tx.set(id, attrs::PAGE_EIGHTHS, i64::from(bd.page_eighths))?;
    tx.set_as(id, BREAKDOWN_ATTR, bd)?;
    tx.link(id, scene, EdgeKind::DerivedFrom)?;
    Ok(id)
}

/// Read the stored breakdown for a scene, if any.
pub fn read_breakdown(graph: &Graph, scene: EntityId) -> Option<SceneBreakdown> {
    graph
        .node(breakdown_id(scene))
        .and_then(|n| n.get_as(BREAKDOWN_ATTR))
}

/// Store a shot list as `logistics.shot_list` derived from `scene`, with one
/// `logistics.shot` child per entry depending on its previs camera. Shots
/// whose camera is no longer listed are deleted.
pub fn write_shot_list(tx: &mut Tx, scene: EntityId, shots: &[ShotEntry]) -> Result<EntityId> {
    require(tx.graph(), scene)?;
    let list = shot_list_id(scene);
    tx.ensure(list, kinds::LOGISTICS_SHOT_LIST)?;
    tx.set(list, attrs::NAME, "Shot list")?;
    tx.link(list, scene, EdgeKind::DerivedFrom)?;
    if let Some(previs) = previs_scene_of(tx.graph(), scene) {
        tx.link(list, previs, EdgeKind::DerivedFrom)?;
    }

    let mut keep: HashSet<EntityId> = HashSet::new();
    for (i, shot) in shots.iter().enumerate() {
        let sid = shot_id(shot.camera);
        keep.insert(sid);
        tx.ensure(sid, kinds::LOGISTICS_SHOT)?;
        tx.set(sid, attrs::NAME, shot.spec.number.clone())?;
        tx.set(sid, attrs::ORDER, i as i64)?;
        shot.spec.write(tx, sid)?;
        tx.set_as(sid, GEAR_ATTR, &shot.gear)?;
        tx.link(list, sid, EdgeKind::Contains)?;
        if tx.graph().contains(shot.camera) {
            tx.link(sid, shot.camera, EdgeKind::DependsOn)?;
        }
    }
    for stale in children_of_kind(tx.graph(), list, kinds::LOGISTICS_SHOT) {
        if !keep.contains(&stale) {
            tx.delete(stale)?;
        }
    }
    Ok(list)
}

/// Read the stored shot list for a scene (empty when none).
pub fn read_shot_list(graph: &Graph, scene: EntityId) -> Vec<ShotEntry> {
    let list = shot_list_id(scene);
    children_of_kind(graph, list, kinds::LOGISTICS_SHOT)
        .into_iter()
        .filter_map(|sid| {
            let node = graph.node(sid)?;
            let spec = ucg_schema::ShotSpec::read(node)?;
            let gear = node.get_as(GEAR_ATTR)?;
            let camera = spec
                .camera
                .or_else(|| graph.targets(sid, EdgeKind::DependsOn).into_iter().next())?;
            Some(ShotEntry {
                spec,
                camera,
                scene,
                gear,
            })
        })
        .collect()
}

/// Store a stripboard as `logistics.schedule` derived from `document`, with
/// one `logistics.day` child per shoot day depending on every scene it
/// contains. Surplus days from a previous, longer schedule are deleted.
pub fn write_schedule(tx: &mut Tx, document: EntityId, schedule: &Schedule) -> Result<EntityId> {
    require(tx.graph(), document)?;
    let sid = schedule_id(document);
    tx.ensure(sid, kinds::LOGISTICS_SCHEDULE)?;
    tx.set(sid, attrs::NAME, "Schedule")?;
    tx.set(sid, WARNINGS_ATTR, schedule.warnings.clone())?;
    tx.set(sid, "day_count", schedule.days.len())?;
    tx.link(sid, document, EdgeKind::DerivedFrom)?;

    let mut keep: HashSet<EntityId> = HashSet::new();
    for day in &schedule.days {
        let did = day_id(sid, day.index);
        keep.insert(did);
        tx.ensure(did, kinds::LOGISTICS_DAY)?;
        tx.set(did, attrs::NAME, format!("Day {}", day.index))?;
        tx.set(did, attrs::DAY, day.index)?;
        tx.set(did, attrs::ORDER, day.index)?;
        tx.set(did, DATE_ATTR, day.date.iso())?;
        tx.set_as(did, STRIPS_ATTR, &day.strips)?;
        tx.set(did, TOTAL_EIGHTHS_ATTR, i64::from(day.total_eighths))?;
        tx.set(did, IS_NIGHT_ATTR, day.is_night)?;
        tx.set(did, CAST_ON_CALL_ATTR, day.cast_on_call.clone())?;
        tx.set(did, LOCATIONS_ATTR, day.locations.clone())?;
        tx.link(sid, did, EdgeKind::Contains)?;

        let scenes: HashSet<EntityId> = day.strips.iter().map(|s| s.scene).collect();
        for scene in day.strips.iter().map(|s| s.scene) {
            if tx.graph().contains(scene) {
                tx.link(did, scene, EdgeKind::DependsOn)?;
            }
        }
        for old in tx.graph().targets(did, EdgeKind::DependsOn) {
            if !scenes.contains(&old) {
                tx.unlink(did, old, EdgeKind::DependsOn)?;
            }
        }
    }
    for stale in children_of_kind(tx.graph(), sid, kinds::LOGISTICS_DAY) {
        if !keep.contains(&stale) {
            tx.delete(stale)?;
        }
    }
    Ok(sid)
}

/// Rebuild a [`Schedule`] from the graph.
pub fn read_schedule(graph: &Graph, document: EntityId) -> Option<Schedule> {
    let sid = schedule_id(document);
    graph.node(sid)?;
    let mut days: Vec<ShootDay> = children_of_kind(graph, sid, kinds::LOGISTICS_DAY)
        .into_iter()
        .filter_map(|did| {
            let n = graph.node(did)?;
            let strips: Vec<Strip> = n.get_as(STRIPS_ATTR).unwrap_or_default();
            Some(ShootDay {
                index: n.get_int(attrs::DAY)? as usize,
                date: Date::parse(n.get_str(DATE_ATTR)?).ok()?,
                total_eighths: n.get_int(TOTAL_EIGHTHS_ATTR).unwrap_or(0) as u32,
                is_night: n.get_bool(IS_NIGHT_ATTR).unwrap_or(false),
                cast_on_call: str_list(graph, did, CAST_ON_CALL_ATTR),
                locations: str_list(graph, did, LOCATIONS_ATTR),
                strips,
            })
        })
        .collect();
    days.sort_by_key(|d| d.index);
    let warnings = str_list(graph, sid, WARNINGS_ATTR);
    Some(Schedule { days, warnings })
}

/// Day node ids of a stored schedule, in day order.
pub fn schedule_day_ids(graph: &Graph, document: EntityId) -> Vec<EntityId> {
    let sid = schedule_id(document);
    let mut days: Vec<(i64, EntityId)> = children_of_kind(graph, sid, kinds::LOGISTICS_DAY)
        .into_iter()
        .map(|d| {
            (
                graph
                    .attr(d, attrs::DAY)
                    .and_then(Value::as_int)
                    .unwrap_or(0),
                d,
            )
        })
        .collect();
    days.sort();
    days.into_iter().map(|(_, d)| d).collect()
}

/// Store a call sheet as `logistics.call_sheet` derived from a day node.
pub fn write_call_sheet(tx: &mut Tx, day: EntityId, sheet: &CallSheet) -> Result<EntityId> {
    require(tx.graph(), day)?;
    let id = call_sheet_id(day);
    tx.ensure(id, kinds::LOGISTICS_CALL_SHEET)?;
    tx.set(id, attrs::NAME, format!("Call sheet day {}", sheet.day))?;
    tx.set(id, attrs::DAY, sheet.day)?;
    tx.set(id, DATE_ATTR, sheet.date.iso())?;
    tx.set_as(id, CALL_SHEET_ATTR, sheet)?;
    tx.link(id, day, EdgeKind::DerivedFrom)?;
    Ok(id)
}

/// Read the stored call sheet for a day node.
pub fn read_call_sheet(graph: &Graph, day: EntityId) -> Option<CallSheet> {
    graph
        .node(call_sheet_id(day))
        .and_then(|n| n.get_as(CALL_SHEET_ATTR))
}

/// Store a take's metadata summary and events as a `logistics.take`
/// contained in `shot`.
pub fn write_take_metadata(
    tx: &mut Tx,
    shot: EntityId,
    take_number: u32,
    summary: &TakeMetadataSummary,
    events: &[MetadataEvent],
) -> Result<EntityId> {
    require(tx.graph(), shot)?;
    let id = take_id(shot, take_number);
    tx.ensure(id, kinds::LOGISTICS_TAKE)?;
    tx.set(id, attrs::NAME, format!("Take {take_number}"))?;
    tx.set(id, TAKE_ATTR, take_number)?;
    tx.set(id, attrs::ORDER, take_number)?;
    tx.set_as(id, attrs::METADATA, summary)?;
    tx.set_as(id, EVENTS_ATTR, &events.to_vec())?;
    tx.link(shot, id, EdgeKind::Contains)?;
    Ok(id)
}

/// Read a take's summary and events.
pub fn read_take_metadata(
    graph: &Graph,
    take: EntityId,
) -> Option<(TakeMetadataSummary, Vec<MetadataEvent>)> {
    let node = graph.node(take)?;
    let summary = node.get_as(attrs::METADATA)?;
    let events = node.get_as(EVENTS_ATTR).unwrap_or_default();
    Some((summary, events))
}

/// Take node ids under a shot, in take order.
pub fn takes_of_shot(graph: &Graph, shot: EntityId) -> Vec<EntityId> {
    children_of_kind(graph, shot, kinds::LOGISTICS_TAKE)
}
