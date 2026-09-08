//! Previs ↔ Unified Context Graph integration.
//!
//! A previs scene is a *derived* entity: its id is
//! `EntityId::derived(script_scene, kinds::PREVIS_SCENE)` and it carries a
//! `DerivedFrom` edge back to the script scene, so the pipeline's reactors
//! can re-run [`sync_scene_from_script`] after every script edit and get
//! the same entities back (idempotent: an unchanged scene produces no ops).
//!
//! Layout under a previs scene (all `Contains` edges):
//!
//! ```text
//! previs.scene            attrs: name, bounds, previs.int_ext
//!   previs.character ×N   derived "character:<NAME>"   transform, name
//!   previs.dialogue_track derived "dialogue_track"     range, stale
//!   previs.camera ×N      derived "camera:<NAME>"      camera, transform, name
//!   previs.light ×N       derived "light:<NAME>"       light, transform, name
//!     previs.stroke ×N    (children of a camera)       previs.stroke_2d / _3d
//! ```

use serde::{Deserialize, Serialize};
use ucg_core::{EdgeKind, EntityId, Graph, RationalTime, TimeRange, Tx, Value};
use ucg_schema::{
    attrs, kinds, Bounds, CameraParams, Component, IntExt, LightParams, SceneHeading, ShotSize,
    Transform,
};

use crate::error::{Error, Result};
use crate::optics::{look_at, vertical_fov};
use crate::stage::{PrimPath, PrimType, Stage};
use crate::stroke::{project_stroke_to_plane, Stroke, Stroke3D};

/// Private attribute: 2D stroke (serialized [`Stroke`]).
pub const ATTR_STROKE_2D: &str = "previs.stroke_2d";
/// Private attribute: projected stroke (serialized [`Stroke3D`]).
pub const ATTR_STROKE_3D: &str = "previs.stroke_3d";
/// Private attribute: projection depth of a stroke in metres.
pub const ATTR_STROKE_DEPTH: &str = "previs.depth_m";
/// Private attribute on the previs scene: `"int"` / `"ext"` used for the default set volume.
pub const ATTR_INT_EXT: &str = "previs.int_ext";

/// Spacing of the default blocking grid along X, in metres.
pub const BLOCKING_SPACING_M: f64 = 1.2;
/// Default stand-in height in metres.
pub const DEFAULT_CHARACTER_HEIGHT_M: f64 = 1.75;
/// Distance of the default camera from the characters' centroid.
pub const DEFAULT_CAMERA_DISTANCE_M: f64 = 4.0;
/// Eye height of the default camera and of its look-at target.
pub const DEFAULT_EYE_HEIGHT_M: f64 = 1.5;
/// Frame rate of the dialogue track range.
pub const DIALOGUE_TRACK_FPS: f64 = 24.0;

/// Default set volume for interiors (width × height × depth, metres).
pub const INT_SET_SIZE_M: [f64; 3] = [8.0, 3.0, 6.0];
/// Default set volume for exteriors.
pub const EXT_SET_SIZE_M: [f64; 3] = [40.0, 10.0, 40.0];

/// What [`sync_scene_from_script`] changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneSyncReport {
    /// Character stand-ins created.
    pub characters_added: usize,
    /// Character stand-ins deleted.
    pub characters_removed: usize,
    /// Whether the dialogue track range changed (the track is then marked stale).
    pub duration_changed: bool,
    /// Whether the default camera was created by this sync.
    pub camera_created: bool,
}

// ---------------------------------------------------------------------------
// Readers
// ---------------------------------------------------------------------------

fn require_kind(graph: &Graph, id: EntityId, kind: &'static str) -> Result<()> {
    let node = graph.require(id)?;
    if node.kind.as_str() != kind {
        return Err(Error::WrongKind {
            entity: id,
            expected: kind,
            actual: node.kind.to_string(),
        });
    }
    Ok(())
}

fn children_of_kind(graph: &Graph, parent: EntityId, kind: &str) -> Vec<EntityId> {
    graph
        .children(parent)
        .into_iter()
        .filter(|c| {
            graph
                .node(*c)
                .map(|n| n.kind.as_str() == kind)
                .unwrap_or(false)
        })
        .collect()
}

/// Character names listed on a script scene (`attrs::CHARACTERS`), in
/// order, de-duplicated, blank entries dropped.
pub fn script_characters(graph: &Graph, script_scene: EntityId) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(list) = graph
        .attr(script_scene, attrs::CHARACTERS)
        .and_then(Value::as_list)
    {
        for v in list {
            if let Some(s) = v.as_str() {
                let s = s.trim();
                if !s.is_empty() && !out.iter().any(|x| x == s) {
                    out.push(s.to_string());
                }
            }
        }
    }
    out
}

/// Interior/exterior of a script scene from its `SceneHeading` component,
/// falling back to `attrs::INT_EXT` / `attrs::HEADING` text.
pub fn script_int_ext(graph: &Graph, script_scene: EntityId) -> IntExt {
    if let Some(h) = SceneHeading::read_from(graph, script_scene) {
        return h.int_ext;
    }
    let text = graph
        .attr(script_scene, attrs::INT_EXT)
        .and_then(Value::as_str)
        .or_else(|| {
            graph
                .attr(script_scene, attrs::HEADING)
                .and_then(Value::as_str)
        })
        .unwrap_or("")
        .to_ascii_uppercase();
    if text.contains("INT") && text.contains("EXT") {
        IntExt::IntExt
    } else if text.contains("EXT") {
        IntExt::Ext
    } else {
        IntExt::Int
    }
}

/// Camera children of a previs scene, in order.
pub fn cameras_of(graph: &Graph, previs_scene: EntityId) -> Vec<EntityId> {
    children_of_kind(graph, previs_scene, kinds::PREVIS_CAMERA)
}

/// Light children of a previs scene, in order.
pub fn lights_of(graph: &Graph, previs_scene: EntityId) -> Vec<EntityId> {
    children_of_kind(graph, previs_scene, kinds::PREVIS_LIGHT)
}

/// Character stand-ins of a previs scene, in order.
pub fn characters_of(graph: &Graph, previs_scene: EntityId) -> Vec<EntityId> {
    children_of_kind(graph, previs_scene, kinds::PREVIS_CHARACTER)
}

/// The dialogue track of a previs scene, if synced.
pub fn dialogue_track_of(graph: &Graph, previs_scene: EntityId) -> Option<EntityId> {
    children_of_kind(graph, previs_scene, kinds::PREVIS_DIALOGUE_TRACK)
        .into_iter()
        .next()
}

/// Strokes stored on a camera: `(id, 2D stroke, projected stroke)`.
pub fn strokes_of(graph: &Graph, camera: EntityId) -> Vec<(EntityId, Stroke, Stroke3D)> {
    children_of_kind(graph, camera, kinds::PREVIS_STROKE)
        .into_iter()
        .filter_map(|id| {
            let n = graph.node(id)?;
            let s2: Stroke = n.get_as(ATTR_STROKE_2D)?;
            let s3: Stroke3D = n.get_as(ATTR_STROKE_3D)?;
            Some((id, s2, s3))
        })
        .collect()
}

/// The script scene a previs scene was derived from.
pub fn script_scene_of(graph: &Graph, previs_scene: EntityId) -> Option<EntityId> {
    graph
        .targets(previs_scene, EdgeKind::DerivedFrom)
        .into_iter()
        .next()
}

// ---------------------------------------------------------------------------
// Writers
// ---------------------------------------------------------------------------

/// Create (or find) the previs scene derived from `script_scene`.
///
/// The id is `EntityId::derived(script_scene, kinds::PREVIS_SCENE)`; the
/// node gets a `DerivedFrom` edge to the script scene and its `name` copied
/// from the script scene's `name` / `heading`. Calling it again is a no-op.
pub fn ensure_previs_scene(tx: &mut Tx, script_scene: EntityId) -> Result<EntityId> {
    let script = tx.graph().require(script_scene)?;
    let name = script
        .get_str(attrs::NAME)
        .map(str::to_string)
        .or_else(|| script.get_str(attrs::HEADING).map(str::to_string))
        .or_else(|| SceneHeading::read(script).map(|h| h.raw))
        .unwrap_or_else(|| format!("Scene {}", script_scene.short()));
    let id = EntityId::derived(script_scene, kinds::PREVIS_SCENE);
    tx.ensure(id, kinds::PREVIS_SCENE)?;
    tx.link(id, script_scene, EdgeKind::DerivedFrom)?;
    tx.set(id, attrs::NAME, name)?;
    Ok(id)
}

/// Default set volume for a scene type: floor at `y = 0`, centred on the origin.
pub fn default_set_bounds(int_ext: IntExt) -> Bounds {
    let s = match int_ext {
        IntExt::Ext => EXT_SET_SIZE_M,
        _ => INT_SET_SIZE_M,
    };
    Bounds {
        min: [-s[0] * 0.5, 0.0, -s[2] * 0.5],
        max: [s[0] * 0.5, s[1], s[2] * 0.5],
    }
}

/// Position of the `i`-th of `n` characters on the default blocking grid:
/// a line along X, `BLOCKING_SPACING_M` apart, centred on the origin.
pub fn blocking_position(i: usize, n: usize) -> [f64; 3] {
    let x = (i as f64 - (n.max(1) as f64 - 1.0) * 0.5) * BLOCKING_SPACING_M;
    [x, 0.0, 0.0]
}

fn union_point(b: &mut Bounds, p: [f64; 3], radius: f64, height: f64) {
    b.min = [
        b.min[0].min(p[0] - radius),
        b.min[1].min(p[1]),
        b.min[2].min(p[2] - radius),
    ];
    b.max = [
        b.max[0].max(p[0] + radius),
        b.max[1].max(p[1] + height),
        b.max[2].max(p[2] + radius),
    ];
}

/// Bring the previs scene in line with its script scene. See the module
/// docs for the layout; the operation is idempotent.
pub fn sync_scene_from_script(
    tx: &mut Tx,
    previs_scene: EntityId,
    script_scene: EntityId,
) -> Result<SceneSyncReport> {
    require_kind(tx.graph(), previs_scene, kinds::PREVIS_SCENE)?;
    tx.graph().require(script_scene)?;
    let mut report = SceneSyncReport::default();

    // ---- what the script says ----
    let wanted = script_characters(tx.graph(), script_scene);
    let int_ext = script_int_ext(tx.graph(), script_scene);
    let est_duration = tx
        .graph()
        .attr(script_scene, attrs::EST_DURATION_S)
        .and_then(Value::as_float)
        .unwrap_or(0.0)
        .max(0.0);
    tx.link(previs_scene, script_scene, EdgeKind::DerivedFrom)?;
    tx.set(
        previs_scene,
        ATTR_INT_EXT,
        if int_ext == IntExt::Ext { "ext" } else { "int" },
    )?;

    // ---- characters ----
    let existing = characters_of(tx.graph(), previs_scene);
    let wanted_ids: Vec<EntityId> = wanted
        .iter()
        .map(|n| EntityId::derived(previs_scene, &format!("character:{n}")))
        .collect();
    for id in &existing {
        if !wanted_ids.contains(id) {
            tx.delete(*id)?;
            report.characters_removed += 1;
        }
    }
    let n = wanted.len();
    for (i, (name, id)) in wanted.iter().zip(&wanted_ids).enumerate() {
        if tx.ensure(*id, kinds::PREVIS_CHARACTER)? {
            report.characters_added += 1;
            let p = blocking_position(i, n);
            Transform::at(p[0], p[1], p[2]).write(tx, *id)?;
            tx.set(*id, "previs.height_m", DEFAULT_CHARACTER_HEIGHT_M)?;
        }
        tx.set(*id, attrs::NAME, name.as_str())?;
        tx.set(*id, attrs::ORDER, i as i64)?;
        tx.link(previs_scene, *id, EdgeKind::Contains)?;
    }

    // ---- bounds ----
    let mut bounds = default_set_bounds(int_ext);
    let mut positions: Vec<[f64; 3]> = Vec::with_capacity(n);
    for id in &wanted_ids {
        let t = Transform::read_from(tx.graph(), *id).unwrap_or_default();
        let h = tx
            .graph()
            .attr(*id, "previs.height_m")
            .and_then(Value::as_float)
            .unwrap_or(DEFAULT_CHARACTER_HEIGHT_M);
        union_point(&mut bounds, t.translate, 0.3, h);
        positions.push(t.translate);
    }
    bounds.write(tx, previs_scene)?;

    // ---- dialogue track ----
    let track = EntityId::derived(previs_scene, "dialogue_track");
    let created = tx.ensure(track, kinds::PREVIS_DIALOGUE_TRACK)?;
    tx.link(previs_scene, track, EdgeKind::Contains)?;
    tx.set(track, attrs::NAME, "Dialogue")?;
    let range = TimeRange::new(
        RationalTime::from_seconds(0.0, DIALOGUE_TRACK_FPS),
        RationalTime::from_seconds(est_duration, DIALOGUE_TRACK_FPS),
    );
    let prev = tx
        .graph()
        .attr(track, attrs::RANGE)
        .and_then(Value::as_range);
    if !created && prev != Some(range) {
        report.duration_changed = true;
        tx.set(track, attrs::STALE, true)?;
    }
    tx.set(track, attrs::RANGE, range)?;

    // ---- default camera ----
    let cam = EntityId::derived(previs_scene, "camera:A");
    if tx.ensure(cam, kinds::PREVIS_CAMERA)? {
        report.camera_created = true;
        let centroid = if positions.is_empty() {
            [0.0, 0.0, 0.0]
        } else {
            let k = positions.len() as f64;
            let s = positions
                .iter()
                .fold([0.0; 3], |a, p| [a[0] + p[0], a[1] + p[1], a[2] + p[2]]);
            [s[0] / k, s[1] / k, s[2] / k]
        };
        let target = [centroid[0], centroid[1] + DEFAULT_EYE_HEIGHT_M, centroid[2]];
        let eye = [target[0], target[1], target[2] + DEFAULT_CAMERA_DISTANCE_M];
        tx.set(cam, attrs::NAME, "A")?;
        CameraParams::default().write(tx, cam)?;
        look_at(eye, target, [0.0, 1.0, 0.0]).write(tx, cam)?;
    }
    tx.link(previs_scene, cam, EdgeKind::Contains)?;

    Ok(report)
}

/// Add (or update) a named camera under a previs scene. The id is derived
/// from the scene and name, so adding the same name twice updates it.
pub fn add_camera(
    tx: &mut Tx,
    previs_scene: EntityId,
    name: &str,
    params: CameraParams,
    transform: Transform,
) -> Result<EntityId> {
    require_kind(tx.graph(), previs_scene, kinds::PREVIS_SCENE)?;
    let id = EntityId::derived(previs_scene, &format!("camera:{name}"));
    tx.ensure(id, kinds::PREVIS_CAMERA)?;
    tx.set(id, attrs::NAME, name)?;
    params.write(tx, id)?;
    transform.write(tx, id)?;
    tx.link(previs_scene, id, EdgeKind::Contains)?;
    Ok(id)
}

/// Add (or update) a named light under a previs scene.
pub fn add_light(
    tx: &mut Tx,
    previs_scene: EntityId,
    name: &str,
    params: LightParams,
    transform: Transform,
) -> Result<EntityId> {
    require_kind(tx.graph(), previs_scene, kinds::PREVIS_SCENE)?;
    let id = EntityId::derived(previs_scene, &format!("light:{name}"));
    tx.ensure(id, kinds::PREVIS_LIGHT)?;
    tx.set(id, attrs::NAME, name)?;
    params.write(tx, id)?;
    transform.write(tx, id)?;
    tx.link(previs_scene, id, EdgeKind::Contains)?;
    Ok(id)
}

/// Store a grease-pencil stroke drawn in a camera's frame as a
/// `previs.stroke` child of the camera, keeping both the 2D samples and
/// their projection onto the plane `depth_m` in front of the camera.
pub fn store_stroke(
    tx: &mut Tx,
    camera: EntityId,
    stroke: &Stroke,
    depth_m: f64,
) -> Result<EntityId> {
    require_kind(tx.graph(), camera, kinds::PREVIS_CAMERA)?;
    let params = CameraParams::read_from(tx.graph(), camera).ok_or(Error::MissingComponent {
        entity: camera,
        component: attrs::CAMERA,
    })?;
    let transform = Transform::read_from(tx.graph(), camera).ok_or(Error::MissingComponent {
        entity: camera,
        component: attrs::TRANSFORM,
    })?;
    let projected = project_stroke_to_plane(stroke, &transform, &params, depth_m);
    let id = tx.create_child(camera, kinds::PREVIS_STROKE)?;
    tx.set(id, ATTR_STROKE_2D, Value::from_serde(stroke)?)?;
    tx.set(id, ATTR_STROKE_3D, Value::from_serde(&projected)?)?;
    tx.set(id, ATTR_STROKE_DEPTH, depth_m)?;
    tx.set(id, "layer", stroke.layer as i64)?;
    tx.set(id, "frame", stroke.frame)?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// Stage
// ---------------------------------------------------------------------------

fn unique_child(stage: &Stage, parent: &PrimPath, name: &str) -> Result<PrimPath> {
    let base = crate::stage::sanitize_identifier(name);
    let mut candidate = parent.child(&base)?;
    let mut n = 1;
    while stage.contains(&candidate) {
        n += 1;
        candidate = parent.child(&format!("{base}_{n}"))?;
    }
    Ok(candidate)
}

/// Build a [`Stage`] from a previs scene:
///
/// ```text
/// /World                      Xform
/// /World/Set                  Scope
/// /World/Set/Floor            Plane   extent = scene bounds
/// /World/Characters/<Name>    Character (extent from previs.height_m)
/// /World/Cameras/<Name>       Camera
/// /World/Lights/<Name>        SphereLight / RectLight / DistantLight
/// ```
pub fn stage_from_graph(graph: &Graph, previs_scene: EntityId) -> Result<Stage> {
    require_kind(graph, previs_scene, kinds::PREVIS_SCENE)?;
    let mut stage = Stage::new();
    stage.default_prim = Some("World".to_string());
    let world = PrimPath::new("/World")?;
    stage.define(&world, PrimType::Xform)?;
    let scene_name = graph
        .node(previs_scene)
        .map(|n| n.name())
        .unwrap_or_default();
    stage
        .get_mut(&world)
        .map(|p| p.set_attr("cine:sceneName", serde_json::json!(scene_name)));

    // Set volume.
    let set = world.child("Set")?;
    stage.define(&set, PrimType::Scope)?;
    let floor = set.child("Floor")?;
    let bounds =
        Bounds::read_from(graph, previs_scene).unwrap_or_else(|| default_set_bounds(IntExt::Int));
    {
        let p = stage.define(&floor, PrimType::Plane)?;
        p.set_extent(bounds.min, bounds.max);
        p.set_attr("axis", serde_json::json!("Y"));
        p.set_attr("width", serde_json::json!(bounds.max[0] - bounds.min[0]));
        p.set_attr("length", serde_json::json!(bounds.max[2] - bounds.min[2]));
    }

    // Characters.
    let chars = world.child("Characters")?;
    stage.define(&chars, PrimType::Xform)?;
    for id in characters_of(graph, previs_scene) {
        let node = graph.require(id)?;
        let name = node.name();
        let t = Transform::read(node).unwrap_or_default();
        let h = node
            .get_float("previs.height_m")
            .unwrap_or(DEFAULT_CHARACTER_HEIGHT_M);
        let path = unique_child(&stage, &chars, &name)?;
        let p = stage.define(&path, PrimType::Character)?;
        p.transform = t;
        p.set_extent([-0.3, 0.0, -0.3], [0.3, h, 0.3]);
        p.set_attr("cine:height", serde_json::json!(h));
        p.set_attr("cine:displayName", serde_json::json!(name));
        p.set_attr("cine:entity", serde_json::json!(id.to_string()));
    }

    // Cameras.
    let cams = world.child("Cameras")?;
    stage.define(&cams, PrimType::Xform)?;
    for id in cameras_of(graph, previs_scene) {
        let node = graph.require(id)?;
        let params = CameraParams::read(node).unwrap_or_default();
        let t = Transform::read(node).unwrap_or_default();
        let path = unique_child(&stage, &cams, &node.name())?;
        stage.add_camera(&path, &params, t)?;
        if let Some(p) = stage.get_mut(&path) {
            p.set_attr("cine:entity", serde_json::json!(id.to_string()));
        }
    }

    // Lights.
    let lights = world.child("Lights")?;
    stage.define(&lights, PrimType::Xform)?;
    for id in lights_of(graph, previs_scene) {
        let node = graph.require(id)?;
        let params = LightParams::read(node).unwrap_or_default();
        let t = Transform::read(node).unwrap_or_default();
        let path = unique_child(&stage, &lights, &node.name())?;
        stage.add_light(&path, &params, t)?;
        if let Some(p) = stage.get_mut(&path) {
            p.set_attr("cine:entity", serde_json::json!(id.to_string()));
        }
    }
    Ok(stage)
}

// ---------------------------------------------------------------------------
// Shot size
// ---------------------------------------------------------------------------

/// Classify a shot by how much of the frame height the subject fills.
///
/// `fill = subject_height / frame_height_at_distance`, where the frame
/// height at the subject is `2·d·tan(vfov/2)`. Thresholds (upper bound of
/// `fill` for each size, using a standing 1.75 m person as the reference):
///
/// | size            | fill      | roughly                          |
/// |-----------------|-----------|----------------------------------|
/// | ExtremeWide     | < 0.30    | figure is a small part of a vista |
/// | Wide            | < 0.60    | full body with lots of context   |
/// | Full            | < 1.15    | head to toe fills the frame      |
/// | MediumWide      | < 1.70    | "cowboy", knees up               |
/// | Medium          | < 2.60    | waist up                         |
/// | MediumCloseUp   | < 4.00    | chest up                         |
/// | CloseUp         | < 7.00    | head and shoulders               |
/// | ExtremeCloseUp  | ≥ 7.00    | eyes / detail                    |
pub fn shot_size_for(
    params: &CameraParams,
    subject_distance_m: f64,
    subject_height_m: f64,
) -> ShotSize {
    let d = subject_distance_m.max(1e-6);
    let frame_h = 2.0 * d * (vertical_fov(params).to_radians() * 0.5).tan();
    if frame_h <= 0.0 || subject_height_m <= 0.0 {
        return ShotSize::ExtremeCloseUp;
    }
    let fill = subject_height_m / frame_h;
    if fill < 0.30 {
        ShotSize::ExtremeWide
    } else if fill < 0.60 {
        ShotSize::Wide
    } else if fill < 1.15 {
        ShotSize::Full
    } else if fill < 1.70 {
        ShotSize::MediumWide
    } else if fill < 2.60 {
        ShotSize::Medium
    } else if fill < 4.00 {
        ShotSize::MediumCloseUp
    } else if fill < 7.00 {
        ShotSize::CloseUp
    } else {
        ShotSize::ExtremeCloseUp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shot_sizes_progress_with_distance() {
        let p = CameraParams::default(); // 35 mm on ALEXA 35: vfov ≈ 30.7°
        let sizes: Vec<ShotSize> = [40.0, 6.0, 3.5, 2.5, 1.8, 1.2, 0.7, 0.4]
            .iter()
            .map(|d| shot_size_for(&p, *d, 1.75))
            .collect();
        assert_eq!(sizes[0], ShotSize::ExtremeWide);
        assert_eq!(sizes[1], ShotSize::Wide);
        assert_eq!(sizes[2], ShotSize::Full);
        assert_eq!(sizes[3], ShotSize::MediumWide);
        assert_eq!(sizes[4], ShotSize::Medium);
        assert_eq!(sizes[5], ShotSize::MediumCloseUp);
        assert_eq!(sizes[6], ShotSize::CloseUp);
        assert_eq!(sizes[7], ShotSize::ExtremeCloseUp);
        for w in sizes.windows(2) {
            assert!(w[0] <= w[1], "{sizes:?}");
        }
    }

    #[test]
    fn blocking_grid_is_centred() {
        assert_eq!(blocking_position(0, 1), [0.0, 0.0, 0.0]);
        let a = blocking_position(0, 3);
        let c = blocking_position(2, 3);
        assert!((a[0] + BLOCKING_SPACING_M).abs() < 1e-12);
        assert!((c[0] - BLOCKING_SPACING_M).abs() < 1e-12);
    }
}
