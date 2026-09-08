//! # cine-previs — storyboard, previs and virtual camera for CINE-OMNI
//!
//! Pure domain modules (no graph dependency):
//! * [`optics`] — sensors, field of view, depth of field, exposure, projection.
//! * [`light`] — Kelvin → RGB, photometry, fixture presets.
//! * [`stage`] — USD-style prim hierarchy with USDA export / import.
//! * [`rig`] — crane forward kinematics, dolly tracks, keyframe animation.
//! * [`stroke`] — grease-pencil strokes projected between frame and world.
//! * [`board`] — 2D storyboard canvas, layers, onion skin, light table.
//!
//! [`graph`] is the only module that touches the Unified Context Graph: it
//! derives a previs scene from a script scene, keeps it in sync, and builds a
//! [`stage::Stage`] for the viewport and USDA export.
//!
//! ```
//! use cine_previs::optics::{horizontal_fov, hyperfocal_m, SensorPreset};
//! use ucg_schema::CameraParams;
//!
//! let mut cam = CameraParams::default();
//! SensorPreset::FullFrame.apply(&mut cam);
//! assert!((horizontal_fov(&cam) - 54.4).abs() < 0.1);
//! assert!((hyperfocal_m(35.0, 2.8, 0.03) - 14.6).abs() < 0.05);
//! ```

pub mod board;
pub mod error;
pub mod graph;
pub mod light;
pub mod optics;
pub mod rig;
pub mod stage;
pub mod stroke;

pub use board::{Board, Brush, Canvas, Frame, Layer, LightTable, OnionSkin, Raster};
pub use error::{Error, Result};
pub use graph::{
    add_camera, add_light, cameras_of, ensure_previs_scene, shot_size_for, stage_from_graph,
    store_stroke, sync_scene_from_script, SceneSyncReport,
};
pub use light::{kelvin_to_rgb, FixturePreset};
pub use optics::{AnamorphicSqueeze, Frustum, Sensor, SensorPreset};
pub use rig::{Animation, CraneRig, DollyTrack, Keyframe, Rig};
pub use stage::{Prim, PrimPath, PrimType, Stage};
pub use stroke::{Ribbon, Stroke, Stroke3D, StrokePoint};
