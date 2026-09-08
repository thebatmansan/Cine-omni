//! Cross-module typed components.

use crate::attrs;
use crate::component::Component;
use serde::{Deserialize, Serialize};
use ucg_core::{EntityId, RationalTime, TimeRange};

// ---------------------------------------------------------------------------
// Spatial
// ---------------------------------------------------------------------------

/// TRS transform in metres / radians (right-handed, Y-up, like USD default).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub translate: [f64; 3],
    /// Quaternion (x, y, z, w).
    pub rotate: [f64; 4],
    pub scale: [f64; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Transform {
            translate: [0.0; 3],
            rotate: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

impl Transform {
    pub fn at(x: f64, y: f64, z: f64) -> Self {
        Transform {
            translate: [x, y, z],
            ..Default::default()
        }
    }
    pub fn to_mat4(&self) -> glam::DMat4 {
        glam::DMat4::from_scale_rotation_translation(
            glam::DVec3::from_array(self.scale),
            glam::DQuat::from_array(self.rotate),
            glam::DVec3::from_array(self.translate),
        )
    }
    pub fn from_mat4(m: &glam::DMat4) -> Self {
        let (s, r, t) = m.to_scale_rotation_translation();
        Transform {
            translate: t.to_array(),
            rotate: r.to_array(),
            scale: s.to_array(),
        }
    }
}

impl Component for Transform {
    const KEY: &'static str = attrs::TRANSFORM;
}

/// Axis-aligned bounds in metres.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Bounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Bounds {
    pub fn size(&self) -> [f64; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }
    pub fn center(&self) -> [f64; 3] {
        [
            (self.max[0] + self.min[0]) * 0.5,
            (self.max[1] + self.min[1]) * 0.5,
            (self.max[2] + self.min[2]) * 0.5,
        ]
    }
}

impl Component for Bounds {
    const KEY: &'static str = attrs::BOUNDS;
}

/// Physical camera parameters shared by previs, on-set metadata, and the NLE.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraParams {
    pub sensor_width_mm: f64,
    pub sensor_height_mm: f64,
    pub focal_length_mm: f64,
    pub f_stop: f64,
    pub focus_distance_m: f64,
    pub shutter_angle_deg: f64,
    pub iso: f64,
    pub white_balance_k: f64,
    pub frame_rate: f64,
    pub resolution: [u32; 2],
    /// Near/far clip in metres (viewport only).
    pub clip: [f64; 2],
}

impl Default for CameraParams {
    fn default() -> Self {
        // ARRI ALEXA 35 open gate-ish defaults with a 35 mm lens.
        CameraParams {
            sensor_width_mm: 27.99,
            sensor_height_mm: 19.22,
            focal_length_mm: 35.0,
            f_stop: 2.8,
            focus_distance_m: 3.0,
            shutter_angle_deg: 180.0,
            iso: 800.0,
            white_balance_k: 5600.0,
            frame_rate: 24.0,
            resolution: [4608, 3164],
            clip: [0.01, 10_000.0],
        }
    }
}

impl Component for CameraParams {
    const KEY: &'static str = attrs::CAMERA;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LightType {
    #[default]
    Point,
    Spot,
    Area,
    Directional,
    Dome,
}

/// Physical light: intensity in lumens (point/spot/area) or lux (directional).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LightParams {
    pub kind: LightType,
    pub intensity: f64,
    pub color_temp_k: f64,
    /// Linear RGB tint multiplied on top of the blackbody color.
    pub tint: [f64; 3],
    pub cone_angle_deg: f64,
    pub softness: f64,
    pub size_m: [f64; 2],
}

impl Default for LightParams {
    fn default() -> Self {
        LightParams {
            kind: LightType::Point,
            intensity: 1600.0,
            color_temp_k: 5600.0,
            tint: [1.0, 1.0, 1.0],
            cone_angle_deg: 45.0,
            softness: 0.2,
            size_m: [1.0, 1.0],
        }
    }
}

impl Component for LightParams {
    const KEY: &'static str = attrs::LIGHT;
}

// ---------------------------------------------------------------------------
// Script
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IntExt {
    #[default]
    Int,
    Ext,
    IntExt,
    Other,
}

/// Parsed scene heading (`INT. KITCHEN - NIGHT`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct SceneHeading {
    pub int_ext: IntExt,
    pub location: String,
    pub time_of_day: String,
    pub raw: String,
}

impl Component for SceneHeading {
    const KEY: &'static str = "scene_heading";
}

// ---------------------------------------------------------------------------
// Editorial
// ---------------------------------------------------------------------------

/// Where a clip sits and what it plays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ClipRef {
    /// `asset.media` entity.
    pub media: Option<EntityId>,
    /// Range inside the media.
    pub source_range: TimeRange,
    /// Start on the parent track (derived; kept for fast queries).
    pub track_start: RationalTime,
}

impl Component for ClipRef {
    const KEY: &'static str = "clip_ref";
}

/// Word-level alignment between script text and a timed medium.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct WordAlignment {
    pub word_index: usize,
    pub word: String,
    pub start_s: f64,
    pub end_s: f64,
    /// 0..1 similarity between script word and transcript word.
    pub confidence: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Alignment {
    /// The medium the words are aligned to (media asset or clip).
    pub target: Option<EntityId>,
    pub words: Vec<WordAlignment>,
    pub stale: bool,
}

impl Component for Alignment {
    const KEY: &'static str = attrs::ALIGNMENT;
}

// ---------------------------------------------------------------------------
// Color
// ---------------------------------------------------------------------------

/// ASC CDL.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cdl {
    pub slope: [f64; 3],
    pub offset: [f64; 3],
    pub power: [f64; 3],
    pub saturation: f64,
}

impl Default for Cdl {
    fn default() -> Self {
        Cdl {
            slope: [1.0; 3],
            offset: [0.0; 3],
            power: [1.0; 3],
            saturation: 1.0,
        }
    }
}

impl Component for Cdl {
    const KEY: &'static str = attrs::CDL;
}

/// Color metadata carried from capture through comp and grade.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorMeta {
    /// OCIO/ACES colorspace name of the source media (e.g. `ARRI LogC4`).
    pub input_colorspace: String,
    /// Working space (default `ACEScg`).
    pub working_colorspace: String,
    /// Display/view (e.g. `Rec.709`, `sRGB`, `Rec.2100-PQ`).
    pub display: String,
    /// `color.lut` entity applied as a look.
    pub look_lut: Option<EntityId>,
    pub cdl: Cdl,
}

impl Default for ColorMeta {
    fn default() -> Self {
        ColorMeta {
            input_colorspace: "ACEScg".into(),
            working_colorspace: "ACEScg".into(),
            display: "Rec.709".into(),
            look_lut: None,
            cdl: Cdl::default(),
        }
    }
}

impl Component for ColorMeta {
    const KEY: &'static str = attrs::COLOR_META;
}

// ---------------------------------------------------------------------------
// Logistics / on-set
// ---------------------------------------------------------------------------

/// One sample of live camera metadata (ingested during capture).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct MetadataFrame {
    pub timecode_frames: i64,
    pub iso: f64,
    pub shutter_angle_deg: f64,
    pub focal_length_mm: f64,
    pub f_stop: f64,
    pub focus_distance_m: f64,
    pub white_balance_k: f64,
    pub tint: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ShotSize {
    ExtremeWide,
    Wide,
    Full,
    MediumWide,
    Medium,
    MediumCloseUp,
    CloseUp,
    ExtremeCloseUp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CameraMove {
    #[default]
    Static,
    Pan,
    Tilt,
    Dolly,
    Crane,
    Handheld,
    Steadicam,
    Zoom,
}

/// Editorial/logistics description of a shot, derived from the previs camera.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShotSpec {
    pub number: String,
    pub size: ShotSize,
    pub movement: CameraMove,
    pub focal_length_mm: f64,
    pub subject_distance_m: f64,
    pub est_duration_s: f64,
    pub description: String,
    pub camera: Option<EntityId>,
    pub scene: Option<EntityId>,
}

impl Default for ShotSpec {
    fn default() -> Self {
        ShotSpec {
            number: "1".into(),
            size: ShotSize::Medium,
            movement: CameraMove::Static,
            focal_length_mm: 35.0,
            subject_distance_m: 3.0,
            est_duration_s: 5.0,
            description: String::new(),
            camera: None,
            scene: None,
        }
    }
}

impl Component for ShotSpec {
    const KEY: &'static str = attrs::SHOT;
}

#[cfg(test)]
mod tests {
    use super::*;
    use ucg_core::prelude::*;

    #[test]
    fn components_round_trip_through_graph() {
        let engine = Engine::new();
        let mut id = EntityId::nil();
        engine
            .transact("t", |tx| {
                id = tx.create("previs.camera")?;
                Transform::at(1.0, 2.0, 3.0).write(tx, id)?;
                CameraParams::default().write(tx, id)?;
                ColorMeta::default().write(tx, id)
            })
            .unwrap();
        let g = engine.read();
        let t = Transform::read_from(&g, id).unwrap();
        assert_eq!(t.translate, [1.0, 2.0, 3.0]);
        let c = CameraParams::read_from(&g, id).unwrap();
        assert_eq!(c.focal_length_mm, 35.0);
        let cm = ColorMeta::read_from(&g, id).unwrap();
        assert_eq!(cm.working_colorspace, "ACEScg");
    }

    #[test]
    fn transform_matrix_round_trip() {
        let t = Transform {
            translate: [1.0, 2.0, 3.0],
            rotate: [
                0.0,
                std::f64::consts::FRAC_1_SQRT_2,
                0.0,
                std::f64::consts::FRAC_1_SQRT_2,
            ],
            scale: [2.0, 2.0, 2.0],
        };
        let back = Transform::from_mat4(&t.to_mat4());
        for i in 0..3 {
            assert!((back.translate[i] - t.translate[i]).abs() < 1e-9);
            assert!((back.scale[i] - t.scale[i]).abs() < 1e-6);
        }
    }
}
