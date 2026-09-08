//! One reactor per module. Each is pure: it reads the change set and the graph
//! and returns plans. See the crate docs for the cascade.

use crate::engine_err;
use std::collections::BTreeSet;
use std::sync::Arc;
use ucg_core::prelude::*;
use ucg_schema::{attrs, kinds, Component};

/// Install every reactor on an engine.
pub fn install_all(engine: &Engine) {
    engine.add_reactor(Arc::new(ScriptReactor));
    engine.add_reactor(Arc::new(PrevisReactor));
    engine.add_reactor(Arc::new(LogisticsReactor));
    engine.add_reactor(Arc::new(TimelineReactor));
    engine.add_reactor(Arc::new(AudioReactor));
    engine.add_reactor(Arc::new(CompReactor));
    engine.add_reactor(Arc::new(ColorReactor));
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// The script scenes affected by a change: scenes touched directly, plus the
/// enclosing scene of any touched script element.
pub fn affected_script_scenes(cs: &ChangeSet, g: &Graph) -> Vec<EntityId> {
    let mut scenes = BTreeSet::new();
    for id in &cs.touched {
        // Deleted nodes are gone from the graph; their parent scene is found via the delta.
        if let Some(n) = g.node(*id) {
            if n.kind.as_str() == kinds::SCRIPT_SCENE {
                scenes.insert(*id);
            } else if n.kind.module() == "script" && kinds::is_script_element(n.kind.as_str()) {
                if let Some(s) = g.ancestor_of_kind(*id, kinds::SCRIPT_SCENE) {
                    scenes.insert(s);
                }
            }
        }
    }
    // Deleted elements: look at the ops for their former parent.
    for op in &cs.delta.ops {
        if let Op::DeleteNode { node, edges } = op {
            if kinds::is_script_element(node.kind.as_str()) {
                for e in edges {
                    let parent_is_scene = g
                        .node(e.from)
                        .map(|n| n.kind.as_str() == kinds::SCRIPT_SCENE)
                        .unwrap_or(false);
                    if e.kind == EdgeKind::Contains && e.to == node.id && parent_is_scene {
                        scenes.insert(e.from);
                    }
                }
            }
        }
    }
    scenes.into_iter().filter(|s| g.contains(*s)).collect()
}

/// Was the change *content* (text / characters / heading / element structure)
/// rather than derived statistics being written back?
fn is_content_change(cs: &ChangeSet, g: &Graph, scene: EntityId) -> bool {
    const STAT_ATTRS: &[&str] = &[
        attrs::WORDS,
        attrs::PAGE_EIGHTHS,
        attrs::EST_DURATION_S,
        attrs::CHARACTERS,
        attrs::LOCATIONS,
        attrs::PROPS,
    ];
    if cs.was_created(scene) {
        return true;
    }
    let scene_changed = cs
        .changed_attrs(scene)
        .iter()
        .any(|a| !STAT_ATTRS.contains(a));
    if scene_changed {
        return true;
    }
    // any element under the scene touched?
    g.descendants(scene)
        .iter()
        .any(|d| cs.is_dirty(*d) || cs.was_created(*d))
        || cs.deleted().any(|_| true) && affected_script_scenes(cs, g).contains(&scene)
}

/// Scheduling constraints stored on the document under `schedule_constraints`
/// (set by the UI / CLI); defaults otherwise.
pub fn read_constraints(
    g: &Graph,
    document: EntityId,
) -> cine_logistics::schedule::ScheduleConstraints {
    g.attr(document, "schedule_constraints")
        .and_then(|v| v.to_serde().ok())
        .unwrap_or_default()
}

fn project_of(g: &Graph, id: EntityId) -> Option<EntityId> {
    g.ancestor_of_kind(id, kinds::PROJECT)
        .or_else(|| g.of_kind(kinds::PROJECT).first().map(|n| n.id))
}

// ---------------------------------------------------------------------------
// Script
// ---------------------------------------------------------------------------

/// Recomputes derived scene statistics after any edit to a scene's elements,
/// and marks ScriptSync alignments stale when dialogue text changes.
pub struct ScriptReactor;

impl Reactor for ScriptReactor {
    fn name(&self) -> &str {
        "script"
    }
    fn interested(&self, cs: &ChangeSet) -> bool {
        cs.touches_module("script.")
    }
    fn react(&self, cs: &ChangeSet, g: &Graph) -> Result<Vec<Plan>> {
        let mut plans = Vec::new();
        for scene in affected_script_scenes(cs, g) {
            if !is_content_change(cs, g, scene) {
                continue;
            }
            let dialogue_changed = g
                .descendants(scene)
                .into_iter()
                .any(|d| cs.attr_changed(d, attrs::TEXT) || cs.was_created(d))
                || cs.delta.ops.iter().any(|op| matches!(op, Op::DeleteNode { node, .. } if node.kind.as_str() == kinds::SCRIPT_DIALOGUE));
            plans.push(Plan::new("recompute scene stats", move |tx| {
                cine_script::graph::recompute_scene_stats(tx, scene)
                    .map_err(|e| engine_err("script", e))?;
                if dialogue_changed {
                    cine_script::graph::mark_alignments_stale(tx, scene)
                        .map_err(|e| engine_err("script", e))?;
                }
                Ok(())
            }));
        }
        Ok(plans)
    }
}

// ---------------------------------------------------------------------------
// Previs
// ---------------------------------------------------------------------------

/// Keeps a derived previs scene per script scene: characters present, set
/// bounds, dialogue track length. Never overwrites artist-placed cameras.
pub struct PrevisReactor;

impl Reactor for PrevisReactor {
    fn name(&self) -> &str {
        "previs"
    }
    fn interested(&self, cs: &ChangeSet) -> bool {
        cs.touches_kind(kinds::SCRIPT_SCENE)
    }
    fn react(&self, cs: &ChangeSet, g: &Graph) -> Result<Vec<Plan>> {
        let mut plans = Vec::new();
        for scene in cs.dirty_of_kind(g, kinds::SCRIPT_SCENE).collect::<Vec<_>>() {
            let relevant = cs.was_created(scene)
                || cs.attr_changed(scene, attrs::CHARACTERS)
                || cs.attr_changed(scene, attrs::EST_DURATION_S)
                || cs.attr_changed(scene, attrs::HEADING)
                || cs.attr_changed(scene, ucg_schema::SceneHeading::KEY);
            if !relevant {
                continue;
            }
            plans.push(Plan::new("sync previs scene", move |tx| {
                let previs = cine_previs::graph::ensure_previs_scene(tx, scene)
                    .map_err(|e| engine_err("previs", e))?;
                cine_previs::graph::sync_scene_from_script(tx, previs, scene)
                    .map_err(|e| engine_err("previs", e))?;
                Ok(())
            }));
        }
        Ok(plans)
    }
}

// ---------------------------------------------------------------------------
// Logistics
// ---------------------------------------------------------------------------

/// Rebuilds breakdown sheets and shot lists for affected scenes, then
/// reschedules the document and regenerates call sheets for days that changed.
pub struct LogisticsReactor;

impl LogisticsReactor {
    fn schedule_plan(document: EntityId) -> Plan {
        Plan::new("reschedule", move |tx| {
            use cine_logistics::schedule::schedule;
            let breakdowns = cine_logistics::breakdown::breakdown_document(tx.graph(), document);
            let constraints = read_constraints(tx.graph(), document);
            let new = schedule(&breakdowns, &constraints);
            let old = cine_logistics::graph::read_schedule(tx.graph(), document);
            let schedule_id = cine_logistics::graph::write_schedule(tx, document, &new)
                .map_err(|e| engine_err("logistics", e))?;
            if let Some(old) = old {
                let diff = cine_logistics::schedule::diff(&old, &new);
                if !diff.moved.is_empty() {
                    tx.set(schedule_id, "last_diff", Value::from_serde(&diff)?)?;
                }
            }
            // Call sheets for every day (idempotent; unchanged days produce no ops).
            let roster = cine_logistics::callsheet::CastRoster::from_breakdowns(&breakdowns);
            let general_call =
                cine_logistics::date::Time::new(7, 0).map_err(|e| engine_err("logistics", e))?;
            let day_ids = tx.graph().children(schedule_id);
            for (i, day) in new.days.iter().enumerate() {
                if let Some(day_id) = day_ids.get(i).copied() {
                    let sheet = cine_logistics::callsheet::build_call_sheet(
                        day,
                        &breakdowns,
                        &roster,
                        general_call,
                    );
                    cine_logistics::graph::write_call_sheet(tx, day_id, &sheet)
                        .map_err(|e| engine_err("logistics", e))?;
                }
            }
            Ok(())
        })
    }
}

impl Reactor for LogisticsReactor {
    fn name(&self) -> &str {
        "logistics"
    }
    fn interested(&self, cs: &ChangeSet) -> bool {
        cs.touches_kind(kinds::SCRIPT_SCENE)
            || cs.touches_kind(kinds::PREVIS_CAMERA)
            || cs.touches_kind(kinds::PREVIS_CHARACTER)
    }
    fn react(&self, cs: &ChangeSet, g: &Graph) -> Result<Vec<Plan>> {
        let mut plans = Vec::new();
        let mut documents = BTreeSet::new();
        let mut scenes: BTreeSet<EntityId> = BTreeSet::new();

        for scene in cs.dirty_of_kind(g, kinds::SCRIPT_SCENE) {
            let relevant = cs.was_created(scene)
                || [
                    attrs::CHARACTERS,
                    attrs::PROPS,
                    attrs::PAGE_EIGHTHS,
                    attrs::EST_DURATION_S,
                    attrs::HEADING,
                    attrs::LOCATIONS,
                    attrs::ORDER,
                ]
                .iter()
                .any(|a| cs.attr_changed(scene, a));
            if relevant {
                scenes.insert(scene);
            }
        }
        // Camera / character changes in previs → shot list of the owning script scene.
        for id in cs.touched.iter().chain(cs.dirty.iter()) {
            if let Some(n) = g.node(*id) {
                if n.kind.as_str() == kinds::PREVIS_CAMERA
                    || n.kind.as_str() == kinds::PREVIS_CHARACTER
                {
                    if let Some(previs) = g.ancestor_of_kind(*id, kinds::PREVIS_SCENE) {
                        for s in g.targets(previs, EdgeKind::DerivedFrom) {
                            if g.node(s)
                                .map(|n| n.kind.as_str() == kinds::SCRIPT_SCENE)
                                .unwrap_or(false)
                            {
                                scenes.insert(s);
                            }
                        }
                    }
                }
            }
        }
        // Deleted scenes → reschedule their document.
        for op in &cs.delta.ops {
            if let Op::DeleteNode { node, edges } = op {
                if node.kind.as_str() == kinds::SCRIPT_SCENE {
                    for e in edges {
                        if e.kind == EdgeKind::Contains && e.to == node.id && g.contains(e.from) {
                            documents.insert(e.from);
                        }
                    }
                }
            }
        }

        for scene in scenes {
            let stats_changed = cs.was_created(scene)
                || [
                    attrs::CHARACTERS,
                    attrs::PROPS,
                    attrs::PAGE_EIGHTHS,
                    attrs::EST_DURATION_S,
                    attrs::HEADING,
                    attrs::LOCATIONS,
                    attrs::ORDER,
                ]
                .iter()
                .any(|a| cs.attr_changed(scene, a));
            plans.push(Plan::new("rebuild breakdown + shot list", move |tx| {
                let bd = cine_logistics::breakdown::breakdown_scene(tx.graph(), scene);
                cine_logistics::graph::write_breakdown(tx, scene, &bd)
                    .map_err(|e| engine_err("logistics", e))?;
                let shots = cine_logistics::shotlist::shot_list_for_scene(tx.graph(), scene);
                cine_logistics::graph::write_shot_list(tx, scene, &shots)
                    .map_err(|e| engine_err("logistics", e))?;
                Ok(())
            }));
            if stats_changed {
                if let Some(doc) = cine_logistics::graph::document_of_scene(g, scene) {
                    documents.insert(doc);
                }
            }
        }
        for doc in documents {
            plans.push(Self::schedule_plan(doc));
        }
        Ok(plans)
    }
}

// ---------------------------------------------------------------------------
// Timeline
// ---------------------------------------------------------------------------

/// Flags every clip synced to a changed scene (or one of its shots) so the
/// editor sees what needs conforming, and re-aligns ScriptSync words.
pub struct TimelineReactor;

impl Reactor for TimelineReactor {
    fn name(&self) -> &str {
        "timeline"
    }
    fn interested(&self, cs: &ChangeSet) -> bool {
        cs.touches_kind(kinds::SCRIPT_SCENE)
            || cs.touches_kind(kinds::LOGISTICS_SHOT)
            || cs.touches_kind(kinds::PREVIS_SHOT)
    }
    fn react(&self, cs: &ChangeSet, g: &Graph) -> Result<Vec<Plan>> {
        let mut plans = Vec::new();
        let mut targets: Vec<(EntityId, String)> = Vec::new();
        for scene in cs.dirty_of_kind(g, kinds::SCRIPT_SCENE) {
            let content = [
                attrs::HEADING,
                attrs::WORDS,
                attrs::CHARACTERS,
                attrs::EST_DURATION_S,
            ]
            .iter()
            .any(|a| cs.attr_changed(scene, a))
                || g.descendants(scene)
                    .iter()
                    .any(|d| cs.attr_changed(*d, attrs::TEXT) || cs.was_created(*d));
            if content {
                let heading = g
                    .node(scene)
                    .and_then(|n| n.get_str(attrs::HEADING))
                    .unwrap_or("")
                    .to_string();
                targets.push((scene, format!("script changed: {heading}")));
            }
        }
        for (target, reason) in targets {
            let clips = cine_timeline::graph::clips_synced_to(g, target);
            if clips.is_empty() {
                continue;
            }
            plans.push(Plan::new("flag clips for conform", move |tx| {
                for clip in &clips {
                    cine_timeline::graph::flag_conform(tx, *clip, &reason)
                        .map_err(|e| engine_err("timeline", e))?;
                }
                Ok(())
            }));
        }
        Ok(plans)
    }
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

/// Keeps a dialogue placeholder track per scene in the DAW session in step
/// with the screenplay (one clip per line, durations from the estimate).
pub struct AudioReactor;

impl Reactor for AudioReactor {
    fn name(&self) -> &str {
        "audio"
    }
    fn interested(&self, cs: &ChangeSet) -> bool {
        cs.touches_kind(kinds::SCRIPT_SCENE) || cs.touches_kind(kinds::SCRIPT_DIALOGUE)
    }
    fn react(&self, cs: &ChangeSet, g: &Graph) -> Result<Vec<Plan>> {
        let mut plans = Vec::new();
        for scene in affected_script_scenes(cs, g) {
            let relevant = cs.was_created(scene)
                || cs.attr_changed(scene, attrs::WORDS)
                || cs.attr_changed(scene, attrs::EST_DURATION_S)
                || g.descendants(scene).iter().any(|d| {
                    g.node(*d)
                        .map(|n| n.kind.as_str() == kinds::SCRIPT_DIALOGUE)
                        .unwrap_or(false)
                        && (cs.attr_changed(*d, attrs::TEXT)
                            || cs.was_created(*d)
                            || cs.attr_changed(*d, attrs::CHARACTER))
                });
            if !relevant {
                continue;
            }
            let Some(project) = project_of(g, scene) else {
                continue;
            };
            plans.push(Plan::new("sync dialogue placeholders", move |tx| {
                let session = cine_audio::ucg::ensure_session(tx, project, None)
                    .map_err(|e| engine_err("audio", e))?;
                let track = cine_audio::ucg::ensure_dialogue_track_for_scene(tx, session, scene)
                    .map_err(|e| engine_err("audio", e))?;
                cine_audio::ucg::sync_dialogue_placeholders(tx, track, scene)
                    .map_err(|e| engine_err("audio", e))?;
                Ok(())
            }));
        }
        Ok(plans)
    }
}

// ---------------------------------------------------------------------------
// Comp
// ---------------------------------------------------------------------------

/// Every shot gets a comp graph; comps bound to changed shots/cameras are
/// marked dirty and their frame ranges refreshed from the shot estimate.
pub struct CompReactor;

impl Reactor for CompReactor {
    fn name(&self) -> &str {
        "comp"
    }
    fn interested(&self, cs: &ChangeSet) -> bool {
        cs.touches_kind(kinds::LOGISTICS_SHOT)
            || cs.touches_kind(kinds::PREVIS_CAMERA)
            || cs.touches_kind(kinds::SCRIPT_SCENE)
    }
    fn react(&self, cs: &ChangeSet, g: &Graph) -> Result<Vec<Plan>> {
        let mut plans = Vec::new();
        // New or changed shots → ensure comp, bind camera, update range.
        for shot in cs
            .dirty_of_kind(g, kinds::LOGISTICS_SHOT)
            .collect::<Vec<_>>()
        {
            let Some(spec) = ucg_schema::ShotSpec::read_from(g, shot) else {
                continue;
            };
            let changed =
                cs.was_created(shot) || cs.attr_changed(shot, attrs::SHOT) || cs.is_dirty(shot);
            if !changed {
                continue;
            }
            let previs_scene = spec
                .camera
                .and_then(|c| g.ancestor_of_kind(c, kinds::PREVIS_SCENE));
            let rate = 24.0;
            let range = TimeRange::new(
                RationalTime::new(0.0, rate),
                RationalTime::from_seconds(spec.est_duration_s, rate),
            );
            let reason = format!("shot {} changed", spec.number);
            plans.push(Plan::new("ensure comp for shot", move |tx| {
                let comp = cine_comp::ucg::ensure_comp_for_shot(tx, shot)
                    .map_err(|e| engine_err("comp", e))?;
                if let (Some(ps), Some(cam)) = (previs_scene, spec.camera) {
                    cine_comp::ucg::bind_previs_camera(tx, comp, ps, cam)
                        .map_err(|e| engine_err("comp", e))?;
                }
                let range_changed = cine_comp::ucg::update_frame_range(tx, comp, range)
                    .map_err(|e| engine_err("comp", e))?;
                if range_changed
                    || !tx
                        .graph()
                        .node(comp)
                        .map(|n| n.get_bool(attrs::STALE).unwrap_or(false))
                        .unwrap_or(false)
                {
                    cine_comp::ucg::mark_dirty(tx, comp, &reason)
                        .map_err(|e| engine_err("comp", e))?;
                }
                Ok(())
            }));
        }
        // Camera moved → comps depending on that camera are stale.
        for cam in cs.touched.iter().copied() {
            if g.node(cam)
                .map(|n| n.kind.as_str() == kinds::PREVIS_CAMERA)
                .unwrap_or(false)
                && cs.any_attr_changed(cam)
            {
                let comps = cine_comp::ucg::comps_depending_on(g, cam);
                if comps.is_empty() {
                    continue;
                }
                plans.push(Plan::new("mark comps dirty (camera)", move |tx| {
                    for c in &comps {
                        cine_comp::ucg::mark_dirty(tx, *c, "previs camera changed")
                            .map_err(|e| engine_err("comp", e))?;
                    }
                    Ok(())
                }));
            }
        }
        Ok(plans)
    }
}

// ---------------------------------------------------------------------------
// Color
// ---------------------------------------------------------------------------

/// Every clip gets a grade; color metadata flows media → clip → grade so a
/// re-conformed cut keeps its LUTs and color spaces.
pub struct ColorReactor;

impl Reactor for ColorReactor {
    fn name(&self) -> &str {
        "color"
    }
    fn interested(&self, cs: &ChangeSet) -> bool {
        cs.touches_kind(kinds::NLE_CLIP) || cs.touches_kind(kinds::ASSET_MEDIA)
    }
    fn react(&self, cs: &ChangeSet, g: &Graph) -> Result<Vec<Plan>> {
        let mut plans = Vec::new();
        let mut clips: BTreeSet<EntityId> = BTreeSet::new();
        for clip in cs.dirty_of_kind(g, kinds::NLE_CLIP) {
            if cs.was_created(clip)
                || cs.attr_changed(clip, attrs::NEEDS_CONFORM)
                || cs.attr_changed(clip, attrs::COLOR_META)
                || cs.is_dirty(clip)
            {
                clips.insert(clip);
            }
        }
        for media in cs.dirty_of_kind(g, kinds::ASSET_MEDIA) {
            if cs.attr_changed(media, attrs::COLOR_META) {
                for clip in g.sources(media, EdgeKind::Binds) {
                    clips.insert(clip);
                }
            }
        }
        for clip in clips {
            let media = cine_timeline::graph::media_of_clip(g, clip);
            plans.push(Plan::new(
                "ensure grade + propagate color meta",
                move |tx| {
                    if let Some(m) = media {
                        cine_color::ucg::propagate_color_meta(tx, m, clip)
                            .map_err(|e| engine_err("color", e))?;
                    }
                    let grade = cine_color::ucg::ensure_grade_for_clip(tx, clip)
                        .map_err(|e| engine_err("color", e))?;
                    cine_color::ucg::propagate_color_meta(tx, clip, grade)
                        .map_err(|e| engine_err("color", e))?;
                    Ok(())
                },
            ));
        }
        Ok(plans)
    }
}
