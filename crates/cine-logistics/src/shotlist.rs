//! Shot lists derived from previs cameras, plus lens-grid analysis.
//!
//! One [`ShotEntry`] is produced per `previs.camera` under the previs scene
//! derived from a script scene. The shot size is classified from the camera's
//! focal length, sensor height and the distance to the nearest
//! `previs.character` (see [`classify_size`] for the thresholds). The gear
//! list estimates support, filters and whether a focus puller is needed from
//! the physical camera parameters.

use crate::breakdown::{csv_field, is_night_time};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use ucg_core::{EdgeKind, EntityId, Graph, Node};
use ucg_schema::{
    attrs, kinds, CameraMove, CameraParams, Component, IntExt, SceneHeading, ShotSize, ShotSpec,
    Transform,
};

/// Assumed standing height of a character, metres.
pub const SUBJECT_HEIGHT_M: f64 = 1.75;

/// Standard cine prime set the lens grid snaps to (mm).
pub const STANDARD_PRIMES: &[f64] = &[
    14.0, 18.0, 21.0, 25.0, 27.0, 32.0, 35.0, 40.0, 50.0, 65.0, 75.0, 100.0, 135.0,
];

/// Shot-size thresholds on `subject_height / frame_height` (the fraction of
/// the vertical field of view a standing 1.75 m subject fills at the subject
/// distance). Calibrated against conventional framings on a Super 35 sensor:
///
/// | size | fraction | frame height at subject |
/// |---|---|---|
/// | Extreme close-up | > 4.0 | < 0.44 m (eyes / mouth) |
/// | Close-up | > 2.2 | < 0.80 m (head and shoulders) |
/// | Medium close-up | > 1.6 | < 1.10 m (chest up) |
/// | Medium | > 1.1 | < 1.60 m (waist up) |
/// | Medium wide | > 0.75 | < 2.30 m (knees up) |
/// | Full | > 0.45 | < 3.90 m (head to toe) |
/// | Wide | > 0.15 | < 11.7 m |
/// | Extreme wide | otherwise | |
pub const SIZE_THRESHOLDS: &[(f64, ShotSize)] = &[
    (4.0, ShotSize::ExtremeCloseUp),
    (2.2, ShotSize::CloseUp),
    (1.6, ShotSize::MediumCloseUp),
    (1.1, ShotSize::Medium),
    (0.75, ShotSize::MediumWide),
    (0.45, ShotSize::Full),
    (0.15, ShotSize::Wide),
];

/// Camera support inferred from the camera move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    #[default]
    Sticks,
    Dolly,
    Crane,
    Handheld,
    Steadicam,
}

impl Support {
    /// Support for a given camera move.
    pub fn for_move(m: CameraMove) -> Support {
        match m {
            CameraMove::Static | CameraMove::Pan | CameraMove::Tilt | CameraMove::Zoom => {
                Support::Sticks
            }
            CameraMove::Dolly => Support::Dolly,
            CameraMove::Crane => Support::Crane,
            CameraMove::Handheld => Support::Handheld,
            CameraMove::Steadicam => Support::Steadicam,
        }
    }

    /// Human label.
    pub fn label(self) -> &'static str {
        match self {
            Support::Sticks => "sticks",
            Support::Dolly => "dolly",
            Support::Crane => "crane",
            Support::Handheld => "handheld",
            Support::Steadicam => "steadicam",
        }
    }
}

impl fmt::Display for Support {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Equipment needed for one shot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GearList {
    pub camera_body: String,
    /// Sensor width / height in mm.
    pub sensor_mm: [f64; 2],
    pub lens_mm: f64,
    pub support: Support,
    /// Filter names (`ND 0.9`, `Polarizer`).
    pub filters: Vec<String>,
    /// True when depth of field is shallow (< 1 m) or the camera moves.
    pub needs_focus_puller: bool,
}

/// A shot on the list: the spec, its previs camera and scene, and gear.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShotEntry {
    pub spec: ShotSpec,
    /// The `previs.camera` the shot derives from.
    pub camera: EntityId,
    /// The `script.scene` the shot covers.
    pub scene: EntityId,
    pub gear: GearList,
}

/// Focal-length usage across a set of shots and the prime kit that covers it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct LensGrid {
    /// Sensor description of the (most common) camera body, e.g. `27.99x19.22mm`.
    pub sensor: String,
    /// Distinct focal lengths actually used, ascending, with shot counts.
    pub focal_lengths: Vec<(f64, usize)>,
    /// Focal lengths snapped to [`STANDARD_PRIMES`], ascending, unique.
    pub recommended_kit: Vec<f64>,
}

/// Classify a shot size from the fraction of the vertical field of view a
/// standing subject fills. See [`SIZE_THRESHOLDS`].
pub fn classify_size(
    focal_mm: f64,
    sensor_height_mm: f64,
    distance_m: f64,
    subject_height_m: f64,
) -> ShotSize {
    let fraction = subject_fraction(focal_mm, sensor_height_mm, distance_m, subject_height_m);
    for (threshold, size) in SIZE_THRESHOLDS {
        if fraction > *threshold {
            return *size;
        }
    }
    ShotSize::ExtremeWide
}

/// `subject_height / frame_height` at the subject distance (pinhole model).
pub fn subject_fraction(
    focal_mm: f64,
    sensor_height_mm: f64,
    distance_m: f64,
    subject_height_m: f64,
) -> f64 {
    if focal_mm <= 0.0 || sensor_height_mm <= 0.0 {
        return 0.0;
    }
    let distance = distance_m.max(0.01);
    let frame_height_m = distance * sensor_height_mm / focal_mm;
    subject_height_m / frame_height_m
}

/// Total depth of field in metres (thin-lens, circle of confusion in mm);
/// `f64::INFINITY` when the far limit is beyond the hyperfocal distance.
pub fn depth_of_field_m(focal_mm: f64, f_stop: f64, distance_m: f64, coc_mm: f64) -> f64 {
    if focal_mm <= 0.0 || f_stop <= 0.0 || coc_mm <= 0.0 || distance_m <= 0.0 {
        return f64::INFINITY;
    }
    let f = focal_mm / 1000.0;
    let c = coc_mm / 1000.0;
    let h = f * f / (f_stop * c) + f;
    let s = distance_m;
    let near = s * (h - f) / (h + s - 2.0 * f);
    if s >= h {
        return f64::INFINITY;
    }
    let far = s * (h - f) / (h - s);
    (far - near).max(0.0)
}

/// Neutral-density filters required to hold the camera's exposure under
/// daylight (sunny-16: EV100 = 15). Returns e.g. `["ND 2.1"]`; empty for
/// interiors, night exteriors, or when less than half a stop is needed.
pub fn nd_filters(cam: &CameraParams, int_ext: IntExt, night: bool) -> Vec<String> {
    if night || !matches!(int_ext, IntExt::Ext | IntExt::IntExt) {
        return Vec::new();
    }
    if cam.f_stop <= 0.0 || cam.iso <= 0.0 || cam.frame_rate <= 0.0 || cam.shutter_angle_deg <= 0.0
    {
        return Vec::new();
    }
    let shutter_s = cam.shutter_angle_deg / 360.0 / cam.frame_rate;
    let scene_ev = 15.0 + (cam.iso / 100.0).log2();
    let camera_ev = (cam.f_stop * cam.f_stop / shutter_s).log2();
    let stops = scene_ev - camera_ev;
    if stops < 0.5 {
        return Vec::new();
    }
    // Density 0.3 per stop, offered in the standard 0.3 increments.
    let density = stops.round() * 0.3;
    vec![format!("ND {density:.1}")]
}

fn letter_suffix(mut i: usize) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (i % 26) as u8) as char);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    s
}

fn camera_move_of(node: &Node) -> CameraMove {
    for key in ["movement", "move", "camera_move"] {
        if let Some(v) = node.get(key) {
            if let Ok(m) = v.to_serde::<CameraMove>() {
                return m;
            }
        }
    }
    CameraMove::Static
}

fn camera_body_of(node: &Node, cam: &CameraParams) -> String {
    for key in ["camera_body", "body"] {
        if let Some(s) = node.get_str(key) {
            return s.to_string();
        }
    }
    if (cam.sensor_width_mm - 27.99).abs() < 0.05 && (cam.sensor_height_mm - 19.22).abs() < 0.05 {
        "ARRI ALEXA 35".to_string()
    } else {
        format!(
            "Custom {:.2}x{:.2}mm",
            cam.sensor_width_mm, cam.sensor_height_mm
        )
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn descendants_of_kind(graph: &Graph, root: EntityId, kind: &str) -> Vec<EntityId> {
    graph
        .descendants(root)
        .into_iter()
        .filter(|id| {
            graph
                .node(*id)
                .map(|n| n.kind.as_str() == kind)
                .unwrap_or(false)
        })
        .collect()
}

/// The previs scene derived from `script_scene`: the conventional derived id
/// first, then any `DerivedFrom` source of kind `previs.scene`.
pub fn previs_scene_of(graph: &Graph, script_scene: EntityId) -> Option<EntityId> {
    let derived = EntityId::derived(script_scene, kinds::PREVIS_SCENE);
    if graph.contains(derived) {
        return Some(derived);
    }
    graph
        .sources(script_scene, EdgeKind::DerivedFrom)
        .into_iter()
        .find(|id| {
            graph
                .node(*id)
                .map(|n| n.kind.as_str() == kinds::PREVIS_SCENE)
                .unwrap_or(false)
        })
}

/// Derive one shot per previs camera under the previs scene of `script_scene`.
/// Returns an empty list when the scene has no previs yet.
pub fn shot_list_for_scene(graph: &Graph, script_scene: EntityId) -> Vec<ShotEntry> {
    let Some(previs) = previs_scene_of(graph, script_scene) else {
        return Vec::new();
    };
    let scene_node = graph.node(script_scene);
    let scene_number = scene_node
        .and_then(|n| n.get_str(attrs::SCENE_NUMBER))
        .map(str::to_string)
        .unwrap_or_else(|| "1".to_string());
    let heading = scene_node.and_then(SceneHeading::read).unwrap_or_else(|| {
        crate::breakdown::parse_heading(
            scene_node
                .and_then(|n| n.get_str(attrs::HEADING))
                .unwrap_or(""),
        )
    });
    let night = is_night_time(&heading.time_of_day);
    let scene_duration = scene_node
        .and_then(|n| n.get_float(attrs::EST_DURATION_S))
        .unwrap_or(0.0);

    let characters: Vec<(String, [f64; 3])> =
        descendants_of_kind(graph, previs, kinds::PREVIS_CHARACTER)
            .into_iter()
            .filter_map(|id| {
                let n = graph.node(id)?;
                let t = Transform::read(n)?;
                Some((
                    n.get_str(attrs::NAME).unwrap_or("").to_string(),
                    t.translate,
                ))
            })
            .collect();

    let cameras = descendants_of_kind(graph, previs, kinds::PREVIS_CAMERA);
    let per_shot_duration = if cameras.is_empty() {
        0.0
    } else {
        scene_duration / cameras.len() as f64
    };

    cameras
        .iter()
        .enumerate()
        .filter_map(|(i, cam_id)| {
            let node = graph.node(*cam_id)?;
            let cam = CameraParams::read(node).unwrap_or_default();
            let cam_pos = Transform::read(node)
                .map(|t| t.translate)
                .unwrap_or([0.0; 3]);
            let nearest = characters
                .iter()
                .map(|(name, pos)| (distance(cam_pos, *pos), name.as_str()))
                .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let (subject_distance, subject) = match nearest {
                Some((d, name)) => (d, Some(name.to_string())),
                None => (cam.focus_distance_m, None),
            };
            let size = classify_size(
                cam.focal_length_mm,
                cam.sensor_height_mm,
                subject_distance,
                SUBJECT_HEIGHT_M,
            );
            let movement = camera_move_of(node);
            let coc_mm = cam.sensor_width_mm / 1500.0;
            let dof = depth_of_field_m(cam.focal_length_mm, cam.f_stop, subject_distance, coc_mm);
            let needs_focus_puller = dof < 1.0 || movement != CameraMove::Static;
            let duration = node.get_float("duration_s").unwrap_or(per_shot_duration);
            let number = format!("{scene_number}{}", letter_suffix(i));
            let description = match &subject {
                Some(name) if !name.is_empty() => {
                    format!(
                        "{} on {}, {:.0}mm {}",
                        size_label(size),
                        name,
                        cam.focal_length_mm,
                        move_label(movement)
                    )
                }
                _ => format!(
                    "{}, {:.0}mm {}",
                    size_label(size),
                    cam.focal_length_mm,
                    move_label(movement)
                ),
            };
            let spec = ShotSpec {
                number,
                size,
                movement,
                focal_length_mm: cam.focal_length_mm,
                subject_distance_m: subject_distance,
                est_duration_s: duration,
                description,
                camera: Some(*cam_id),
                scene: Some(script_scene),
            };
            let gear = GearList {
                camera_body: camera_body_of(node, &cam),
                sensor_mm: [cam.sensor_width_mm, cam.sensor_height_mm],
                lens_mm: cam.focal_length_mm,
                support: Support::for_move(movement),
                filters: nd_filters(&cam, heading.int_ext, night),
                needs_focus_puller,
            };
            Some(ShotEntry {
                spec,
                camera: *cam_id,
                scene: script_scene,
                gear,
            })
        })
        .collect()
}

/// Short label for a shot size (`CU`, `MW`).
pub fn size_label(size: ShotSize) -> &'static str {
    match size {
        ShotSize::ExtremeWide => "EWS",
        ShotSize::Wide => "WS",
        ShotSize::Full => "FS",
        ShotSize::MediumWide => "MWS",
        ShotSize::Medium => "MS",
        ShotSize::MediumCloseUp => "MCU",
        ShotSize::CloseUp => "CU",
        ShotSize::ExtremeCloseUp => "ECU",
    }
}

/// Lower-case label for a camera move.
pub fn move_label(m: CameraMove) -> &'static str {
    match m {
        CameraMove::Static => "static",
        CameraMove::Pan => "pan",
        CameraMove::Tilt => "tilt",
        CameraMove::Dolly => "dolly",
        CameraMove::Crane => "crane",
        CameraMove::Handheld => "handheld",
        CameraMove::Steadicam => "steadicam",
        CameraMove::Zoom => "zoom",
    }
}

/// Snap a focal length to the nearest standard prime (ties go to the shorter lens).
pub fn snap_to_prime(mm: f64) -> f64 {
    let mut best = STANDARD_PRIMES[0];
    let mut best_d = f64::INFINITY;
    for &p in STANDARD_PRIMES {
        let d = (p - mm).abs();
        if d < best_d {
            best_d = d;
            best = p;
        }
    }
    best
}

/// Aggregate focal-length usage across shots into a [`LensGrid`].
pub fn lens_grid(shots: &[ShotEntry]) -> LensGrid {
    // Key by tenths of a millimetre so float noise does not split entries.
    let mut counts: BTreeMap<i64, (f64, usize)> = BTreeMap::new();
    let mut sensors: BTreeMap<String, usize> = BTreeMap::new();
    for s in shots {
        let key = (s.spec.focal_length_mm * 10.0).round() as i64;
        let e = counts.entry(key).or_insert((s.spec.focal_length_mm, 0));
        e.1 += 1;
        *sensors
            .entry(format!(
                "{:.2}x{:.2}mm",
                s.gear.sensor_mm[0], s.gear.sensor_mm[1]
            ))
            .or_default() += 1;
    }
    let focal_lengths: Vec<(f64, usize)> = counts.values().copied().collect();
    let mut kit: Vec<i64> = focal_lengths
        .iter()
        .map(|(mm, _)| (snap_to_prime(*mm) * 10.0).round() as i64)
        .collect();
    kit.sort_unstable();
    kit.dedup();
    let sensor = sensors
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(s, _)| s.clone())
        .unwrap_or_default();
    LensGrid {
        sensor,
        focal_lengths,
        recommended_kit: kit.into_iter().map(|k| k as f64 / 10.0).collect(),
    }
}

/// Markdown table of a shot list.
pub fn shots_to_markdown(shots: &[ShotEntry]) -> String {
    let mut out = String::from(
        "| Shot | Size | Move | Lens | Dist | Dur | Support | Filters | Focus | Description |\n|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for s in shots {
        out.push_str(&format!(
            "| {} | {} | {} | {:.0}mm | {:.1}m | {:.1}s | {} | {} | {} | {} |\n",
            s.spec.number,
            size_label(s.spec.size),
            move_label(s.spec.movement),
            s.spec.focal_length_mm,
            s.spec.subject_distance_m,
            s.spec.est_duration_s,
            s.gear.support,
            s.gear.filters.join(", "),
            if s.gear.needs_focus_puller {
                "1st AC"
            } else {
                "-"
            },
            s.spec.description
        ));
    }
    out
}

/// CSV export of a shot list.
pub fn shots_to_csv(shots: &[ShotEntry]) -> String {
    let mut out = String::from(
        "shot,scene,camera,size,movement,focal_mm,distance_m,duration_s,body,support,filters,focus_puller,description\n",
    );
    for s in shots {
        let row = [
            s.spec.number.clone(),
            s.scene.to_string(),
            s.camera.to_string(),
            size_label(s.spec.size).to_string(),
            move_label(s.spec.movement).to_string(),
            format!("{:.1}", s.spec.focal_length_mm),
            format!("{:.2}", s.spec.subject_distance_m),
            format!("{:.2}", s.spec.est_duration_s),
            s.gear.camera_body.clone(),
            s.gear.support.to_string(),
            s.gear.filters.join("; "),
            s.gear.needs_focus_puller.to_string(),
            s.spec.description.clone(),
        ];
        out.push_str(
            &row.iter()
                .map(|f| csv_field(f))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    out
}

impl LensGrid {
    /// Markdown summary.
    pub fn to_markdown(&self) -> String {
        let mut out = format!("Sensor: {}\n\n| Focal | Shots |\n|---|---|\n", self.sensor);
        for (mm, n) in &self.focal_lengths {
            out.push_str(&format!("| {mm:.1}mm | {n} |\n"));
        }
        let kit: Vec<String> = self
            .recommended_kit
            .iter()
            .map(|k| format!("{k:.0}mm"))
            .collect();
        out.push_str(&format!("\nRecommended kit: {}\n", kit.join(", ")));
        out
    }

    /// CSV (`focal_mm,shots,prime_mm`).
    pub fn to_csv(&self) -> String {
        let mut out = String::from("focal_mm,shots,prime_mm\n");
        for (mm, n) in &self.focal_lengths {
            out.push_str(&format!("{mm:.1},{n},{:.0}\n", snap_to_prime(*mm)));
        }
        out
    }
}

/// Convenience: shot list plus lens grid for a whole document.
pub fn shot_list_for_document(graph: &Graph, document: EntityId) -> Vec<ShotEntry> {
    graph
        .children(document)
        .into_iter()
        .filter(|id| {
            graph
                .node(*id)
                .map(|n| n.kind.as_str() == kinds::SCRIPT_SCENE)
                .unwrap_or(false)
        })
        .flat_map(|scene| shot_list_for_scene(graph, scene))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const S35_H: f64 = 19.22;

    #[test]
    fn size_thresholds_match_conventional_framings() {
        assert_eq!(
            classify_size(35.0, S35_H, 1.0, SUBJECT_HEIGHT_M),
            ShotSize::CloseUp
        );
        assert_eq!(
            classify_size(50.0, S35_H, 1.0, SUBJECT_HEIGHT_M),
            ShotSize::ExtremeCloseUp
        );
        assert_eq!(
            classify_size(85.0, S35_H, 1.0, SUBJECT_HEIGHT_M),
            ShotSize::ExtremeCloseUp
        );
        assert_eq!(
            classify_size(35.0, S35_H, 3.0, SUBJECT_HEIGHT_M),
            ShotSize::MediumWide
        );
        assert_eq!(
            classify_size(50.0, S35_H, 3.0, SUBJECT_HEIGHT_M),
            ShotSize::Medium
        );
        assert_eq!(
            classify_size(85.0, S35_H, 3.0, SUBJECT_HEIGHT_M),
            ShotSize::CloseUp
        );
        assert_eq!(
            classify_size(35.0, S35_H, 12.0, SUBJECT_HEIGHT_M),
            ShotSize::Wide
        );
        assert_eq!(
            classify_size(50.0, S35_H, 12.0, SUBJECT_HEIGHT_M),
            ShotSize::Wide
        );
        assert_eq!(
            classify_size(85.0, S35_H, 12.0, SUBJECT_HEIGHT_M),
            ShotSize::Full
        );
        assert_eq!(
            classify_size(18.0, S35_H, 40.0, SUBJECT_HEIGHT_M),
            ShotSize::ExtremeWide
        );
        assert_eq!(
            classify_size(0.0, S35_H, 1.0, SUBJECT_HEIGHT_M),
            ShotSize::ExtremeWide
        );
    }

    #[test]
    fn prime_snapping() {
        assert_eq!(snap_to_prime(85.0), 75.0);
        assert_eq!(snap_to_prime(90.0), 100.0);
        assert_eq!(snap_to_prime(36.0), 35.0);
        assert_eq!(snap_to_prime(8.0), 14.0);
        assert_eq!(snap_to_prime(400.0), 135.0);
        assert_eq!(snap_to_prime(33.5), 32.0);
    }

    #[test]
    fn depth_of_field_is_shallow_wide_open_and_infinite_stopped_down() {
        let coc = 27.99 / 1500.0;
        let shallow = depth_of_field_m(85.0, 1.4, 1.0, coc);
        assert!(shallow < 0.1, "{shallow}");
        let deep = depth_of_field_m(18.0, 11.0, 5.0, coc);
        assert!(deep.is_infinite());
        let mid = depth_of_field_m(35.0, 2.8, 3.0, coc);
        assert!(mid > 0.5 && mid < 3.0, "{mid}");
    }

    #[test]
    fn nd_filters_only_for_daylight_exteriors() {
        let cam = CameraParams::default();
        assert!(nd_filters(&cam, IntExt::Int, false).is_empty());
        assert!(nd_filters(&cam, IntExt::Ext, true).is_empty());
        let nd = nd_filters(&cam, IntExt::Ext, false);
        assert_eq!(nd.len(), 1);
        // ISO 800, 180° @ 24 fps, f/2.8 in full sun is ~9 stops over.
        assert_eq!(nd[0], "ND 2.7");
        let stopped = CameraParams {
            f_stop: 22.0,
            iso: 100.0,
            ..CameraParams::default()
        };
        assert!(nd_filters(&stopped, IntExt::Ext, false).is_empty());
    }

    #[test]
    fn letter_suffixes() {
        assert_eq!(letter_suffix(0), "A");
        assert_eq!(letter_suffix(25), "Z");
        assert_eq!(letter_suffix(26), "AA");
        assert_eq!(letter_suffix(27), "AB");
    }

    #[test]
    fn support_from_move() {
        assert_eq!(Support::for_move(CameraMove::Pan), Support::Sticks);
        assert_eq!(Support::for_move(CameraMove::Dolly), Support::Dolly);
        assert_eq!(Support::for_move(CameraMove::Crane), Support::Crane);
        assert_eq!(Support::for_move(CameraMove::Handheld), Support::Handheld);
    }
}
