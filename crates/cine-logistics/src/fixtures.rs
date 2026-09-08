//! Hand-built sample documents for tests and demos.
//!
//! Builds `script.document` / `script.scene` nodes with the attributes the
//! script module produces, and previs scenes with cameras and characters, so
//! logistics can be exercised without the script or previs crates.

use crate::error::Result;
use ucg_core::{EdgeKind, EntityId, Tx};
use ucg_schema::{attrs, kinds, CameraMove, CameraParams, Component, Transform};

/// Description of one scene to build.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneSpec {
    pub number: String,
    pub heading: String,
    pub cast: Vec<String>,
    pub props: Vec<String>,
    pub page_eighths: u32,
    pub duration_s: f64,
}

impl SceneSpec {
    /// Convenience constructor.
    pub fn new(
        number: &str,
        heading: &str,
        cast: &[&str],
        props: &[&str],
        page_eighths: u32,
        duration_s: f64,
    ) -> Self {
        SceneSpec {
            number: number.to_string(),
            heading: heading.to_string(),
            cast: cast.iter().map(|s| s.to_string()).collect(),
            props: props.iter().map(|s| s.to_string()).collect(),
            page_eighths,
            duration_s,
        }
    }
}

/// A built document and its scene ids in order.
#[derive(Clone, Debug, PartialEq)]
pub struct SampleDocument {
    pub document: EntityId,
    pub scenes: Vec<EntityId>,
}

/// The six-scene sample script used throughout the tests.
pub fn sample_scenes() -> Vec<SceneSpec> {
    vec![
        SceneSpec::new(
            "1",
            "INT. KITCHEN - DAY",
            &["ANNA", "BEN"],
            &["KNIFE", "COFFEE MUG"],
            6,
            60.0,
        ),
        SceneSpec::new(
            "2",
            "EXT. STREET - DAY",
            &["ANNA"],
            &["CAR", "UMBRELLA"],
            4,
            45.0,
        ),
        SceneSpec::new(
            "3",
            "INT. KITCHEN - NIGHT",
            &["ANNA", "BEN", "CARLA"],
            &["WINE BOTTLE"],
            10,
            90.0,
        ),
        SceneSpec::new(
            "4",
            "EXT. FOREST - NIGHT",
            &["BEN"],
            &["DOG", "FLASHLIGHT"],
            8,
            80.0,
        ),
        SceneSpec::new(
            "5",
            "INT. WAREHOUSE - DAY",
            &["CARLA", "DAVE"],
            &["EXPLOSION CHARGE", "GREEN SCREEN"],
            12,
            100.0,
        ),
        SceneSpec::new(
            "6",
            "EXT. STREET - NIGHT",
            &["ANNA", "DAVE"],
            &["TAXI"],
            3,
            30.0,
        ),
    ]
}

/// Add a `script.scene` under `document` with a stable id derived from the
/// document and scene number.
pub fn add_scene(
    tx: &mut Tx,
    document: EntityId,
    order: usize,
    spec: &SceneSpec,
) -> Result<EntityId> {
    let id = EntityId::derived(document, &format!("scene:{}", spec.number));
    tx.ensure(id, kinds::SCRIPT_SCENE)?;
    tx.set(id, attrs::HEADING, spec.heading.clone())?;
    tx.set(id, attrs::SCENE_NUMBER, spec.number.clone())?;
    tx.set(id, attrs::ORDER, order)?;
    let heading = crate::breakdown::parse_heading(&spec.heading);
    heading.write(tx, id)?;
    tx.set(id, attrs::CHARACTERS, spec.cast.clone())?;
    tx.set(id, attrs::LOCATIONS, vec![heading.location.clone()])?;
    tx.set(id, attrs::PROPS, spec.props.clone())?;
    tx.set(id, attrs::PAGE_EIGHTHS, i64::from(spec.page_eighths))?;
    tx.set(id, attrs::EST_DURATION_S, spec.duration_s)?;
    tx.link(document, id, EdgeKind::Contains)?;
    Ok(id)
}

/// Build a document from scene specs. `key` makes the document id stable.
pub fn build_document(tx: &mut Tx, key: &str, scenes: &[SceneSpec]) -> Result<SampleDocument> {
    let document = EntityId::from_key(&format!("fixture:{key}"));
    tx.ensure(document, kinds::SCRIPT_DOCUMENT)?;
    tx.set(document, attrs::NAME, key)?;
    let mut ids = Vec::with_capacity(scenes.len());
    for (i, s) in scenes.iter().enumerate() {
        ids.push(add_scene(tx, document, i, s)?);
    }
    Ok(SampleDocument {
        document,
        scenes: ids,
    })
}

/// Build the standard six-scene sample document.
pub fn build_sample_document(tx: &mut Tx) -> Result<SampleDocument> {
    build_document(tx, "sample", &sample_scenes())
}

/// A previs camera to place in front of the characters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraSetup {
    pub focal_mm: f64,
    /// Distance from the first character along +Z, metres.
    pub distance_m: f64,
    pub movement: CameraMove,
}

impl CameraSetup {
    /// A static camera.
    pub fn new(focal_mm: f64, distance_m: f64) -> Self {
        CameraSetup {
            focal_mm,
            distance_m,
            movement: CameraMove::Static,
        }
    }
}

/// Add a `previs.scene` derived from `script_scene` with the given cameras
/// and characters. Characters stand 0.3 m apart along +X starting at the
/// origin; cameras sit on the Z axis so the first character is exactly
/// `distance_m` away. Returns the previs scene id.
pub fn add_previs(
    tx: &mut Tx,
    script_scene: EntityId,
    cameras: &[CameraSetup],
    characters: &[&str],
) -> Result<EntityId> {
    let previs = EntityId::derived(script_scene, kinds::PREVIS_SCENE);
    tx.ensure(previs, kinds::PREVIS_SCENE)?;
    tx.link(previs, script_scene, EdgeKind::DerivedFrom)?;
    for (i, name) in characters.iter().enumerate() {
        let id = EntityId::derived(previs, &format!("character:{name}"));
        tx.ensure(id, kinds::PREVIS_CHARACTER)?;
        tx.set(id, attrs::NAME, *name)?;
        tx.set(id, attrs::ORDER, i)?;
        Transform::at(0.3 * i as f64, 0.0, 0.0).write(tx, id)?;
        tx.link(previs, id, EdgeKind::Contains)?;
    }
    for (i, cam) in cameras.iter().enumerate() {
        let id = EntityId::derived(previs, &format!("camera:{i}"));
        tx.ensure(id, kinds::PREVIS_CAMERA)?;
        tx.set(id, attrs::NAME, format!("Cam {}", i + 1))?;
        tx.set(id, attrs::ORDER, i)?;
        CameraParams {
            focal_length_mm: cam.focal_mm,
            focus_distance_m: cam.distance_m,
            ..Default::default()
        }
        .write(tx, id)?;
        Transform::at(0.0, 0.0, cam.distance_m).write(tx, id)?;
        tx.set_as(id, "movement", &cam.movement)?;
        tx.link(previs, id, EdgeKind::Contains)?;
    }
    Ok(previs)
}

/// Camera ids under a previs scene built by [`add_previs`], in order.
pub fn previs_camera_ids(previs: EntityId, count: usize) -> Vec<EntityId> {
    (0..count)
        .map(|i| EntityId::derived(previs, &format!("camera:{i}")))
        .collect()
}
