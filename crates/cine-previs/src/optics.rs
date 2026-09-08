//! Physically accurate camera optics.
//!
//! Conventions (shared by every module of this crate):
//! * Scene units are metres, right-handed, **Y up** (the USD default).
//! * A camera looks down its local **-Z** axis, +Y is up, +X is right
//!   (OpenGL / USD camera convention).
//! * "Frame coordinates" are normalized image coordinates in `0..1` with the
//!   origin at the **top-left** of the frame and `y` growing downwards, which
//!   is what the 2D storyboard canvas and the grease-pencil tools use.
//! * NDC is OpenGL-style: `x, y, z ∈ [-1, 1]` with `y` up.
//!
//! All focal lengths and sensor sizes are in millimetres; all distances in
//! metres unless the name says otherwise.

use glam::{DMat4, DQuat, DVec3, DVec4};
use serde::{Deserialize, Serialize};
use ucg_schema::{CameraParams, Transform};

use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// Sensors
// ---------------------------------------------------------------------------

/// Physical sensor (or film gate) dimensions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sensor {
    /// Active width in millimetres.
    pub width_mm: f64,
    /// Active height in millimetres.
    pub height_mm: f64,
    /// Human readable name (`"ARRI ALEXA 35"`).
    pub name: String,
}

impl Sensor {
    /// Build a custom sensor.
    pub fn new(name: impl Into<String>, width_mm: f64, height_mm: f64) -> Self {
        Sensor {
            width_mm,
            height_mm,
            name: name.into(),
        }
    }
    /// Diagonal in millimetres.
    pub fn diagonal_mm(&self) -> f64 {
        (self.width_mm * self.width_mm + self.height_mm * self.height_mm).sqrt()
    }
    /// Width / height.
    pub fn aspect(&self) -> f64 {
        self.width_mm / self.height_mm
    }
    /// Crop factor relative to full frame (36 × 24, diagonal 43.27 mm).
    pub fn crop_factor(&self) -> f64 {
        crop_factor(self.width_mm, self.height_mm)
    }
    /// The sensor described by a set of camera parameters.
    pub fn from_params(p: &CameraParams) -> Self {
        Sensor::new("camera", p.sensor_width_mm, p.sensor_height_mm)
    }
}

/// Well known cinema sensors and film gates (active area, open gate / full
/// sensor unless noted). Values are manufacturer figures rounded to 0.01 mm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorPreset {
    /// ARRI ALEXA 35, 4.6K 3:2 open gate.
    ArriAlexa35,
    /// ARRI ALEXA Mini LF, LF open gate.
    ArriAlexaMiniLf,
    /// ARRI ALEXA 65, 6.5K open gate.
    ArriAlexa65,
    /// RED V-RAPTOR 8K VV, full sensor.
    RedVRaptor8kVv,
    /// Sony VENICE 2, 8.6K full frame 3:2.
    SonyVenice2,
    /// Generic 35 mm full frame stills sensor.
    FullFrame,
    /// Super 35 film gate (4-perf full aperture).
    Super35,
    /// Micro Four Thirds.
    MicroFourThirds,
    /// Super 16 film gate.
    Super16,
    /// IMAX 15/70 film gate.
    Imax1570,
}

impl SensorPreset {
    /// Every preset, in display order.
    pub const ALL: [SensorPreset; 10] = [
        SensorPreset::ArriAlexa35,
        SensorPreset::ArriAlexaMiniLf,
        SensorPreset::ArriAlexa65,
        SensorPreset::RedVRaptor8kVv,
        SensorPreset::SonyVenice2,
        SensorPreset::FullFrame,
        SensorPreset::Super35,
        SensorPreset::MicroFourThirds,
        SensorPreset::Super16,
        SensorPreset::Imax1570,
    ];

    /// The physical dimensions of this preset.
    pub fn sensor(self) -> Sensor {
        let (name, w, h) = match self {
            SensorPreset::ArriAlexa35 => ("ARRI ALEXA 35", 27.99, 19.22),
            SensorPreset::ArriAlexaMiniLf => ("ARRI ALEXA Mini LF", 36.70, 25.54),
            SensorPreset::ArriAlexa65 => ("ARRI ALEXA 65", 54.12, 25.58),
            SensorPreset::RedVRaptor8kVv => ("RED V-RAPTOR 8K VV", 40.96, 21.60),
            SensorPreset::SonyVenice2 => ("Sony VENICE 2", 36.2, 24.1),
            SensorPreset::FullFrame => ("Full Frame", 36.0, 24.0),
            SensorPreset::Super35 => ("Super 35", 24.89, 18.66),
            SensorPreset::MicroFourThirds => ("Micro Four Thirds", 17.3, 13.0),
            SensorPreset::Super16 => ("Super 16", 12.52, 7.41),
            SensorPreset::Imax1570 => ("IMAX 15/70", 70.41, 52.63),
        };
        Sensor::new(name, w, h)
    }

    /// Apply this sensor to a set of camera parameters (keeps everything else).
    pub fn apply(self, params: &mut CameraParams) {
        let s = self.sensor();
        params.sensor_width_mm = s.width_mm;
        params.sensor_height_mm = s.height_mm;
    }
}

/// Full-frame reference diagonal (36 × 24 mm).
pub const FULL_FRAME_DIAGONAL_MM: f64 = 43.266_615_305_567_875;

/// Crop factor of a sensor relative to full frame, by diagonal.
pub fn crop_factor(width_mm: f64, height_mm: f64) -> f64 {
    FULL_FRAME_DIAGONAL_MM / (width_mm * width_mm + height_mm * height_mm).sqrt()
}

/// Focal length on full frame that gives the same diagonal field of view.
pub fn equivalent_focal_ff(focal_mm: f64, width_mm: f64, height_mm: f64) -> f64 {
    focal_mm * crop_factor(width_mm, height_mm)
}

// ---------------------------------------------------------------------------
// Field of view
// ---------------------------------------------------------------------------

/// Angle of view in degrees for one sensor dimension: `2·atan(d / 2f)`.
pub fn fov_deg(sensor_dim_mm: f64, focal_mm: f64) -> f64 {
    2.0 * (sensor_dim_mm / (2.0 * focal_mm)).atan().to_degrees()
}

/// Focal length (mm) that yields `fov_deg` across a sensor dimension.
pub fn focal_for_fov(sensor_dim_mm: f64, fov_deg: f64) -> f64 {
    sensor_dim_mm / (2.0 * (fov_deg.to_radians() * 0.5).tan())
}

/// Horizontal angle of view (degrees) for spherical lenses.
pub fn horizontal_fov(p: &CameraParams) -> f64 {
    fov_deg(p.sensor_width_mm, p.focal_length_mm)
}

/// Vertical angle of view (degrees).
pub fn vertical_fov(p: &CameraParams) -> f64 {
    fov_deg(p.sensor_height_mm, p.focal_length_mm)
}

/// Diagonal angle of view (degrees).
pub fn diagonal_fov(p: &CameraParams) -> f64 {
    fov_deg(Sensor::from_params(p).diagonal_mm(), p.focal_length_mm)
}

/// Anamorphic squeeze factors in common use. The squeeze widens the
/// horizontal field of view by the factor (the image is de-squeezed in post),
/// leaving the vertical field of view untouched.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnamorphicSqueeze {
    /// Spherical lens, no squeeze (1.0).
    #[default]
    Spherical,
    /// 1.33× (16:9 sensors → 2.39:1).
    X1_33,
    /// 1.5× (3:2 open gate → 2.39:1 / 2.66:1).
    X1_5,
    /// 1.8× (large format open gate).
    X1_8,
    /// 2.0× classic anamorphic.
    X2,
    /// Any other factor (some vintage lenses are 1.25×, 1.35×...).
    Custom(f64),
}

impl AnamorphicSqueeze {
    /// The horizontal expansion factor.
    pub fn factor(self) -> f64 {
        match self {
            AnamorphicSqueeze::Spherical => 1.0,
            AnamorphicSqueeze::X1_33 => 1.33,
            AnamorphicSqueeze::X1_5 => 1.5,
            AnamorphicSqueeze::X1_8 => 1.8,
            AnamorphicSqueeze::X2 => 2.0,
            AnamorphicSqueeze::Custom(f) => f.max(1e-6),
        }
    }
}

/// Effective (de-squeezed) sensor width in millimetres.
pub fn effective_sensor_width_mm(p: &CameraParams, squeeze: AnamorphicSqueeze) -> f64 {
    p.sensor_width_mm * squeeze.factor()
}

/// Horizontal angle of view accounting for anamorphic squeeze.
pub fn horizontal_fov_anamorphic(p: &CameraParams, squeeze: AnamorphicSqueeze) -> f64 {
    fov_deg(effective_sensor_width_mm(p, squeeze), p.focal_length_mm)
}

/// Delivered aspect ratio (width / height) after de-squeeze.
pub fn delivered_aspect(p: &CameraParams, squeeze: AnamorphicSqueeze) -> f64 {
    effective_sensor_width_mm(p, squeeze) / p.sensor_height_mm
}

// ---------------------------------------------------------------------------
// Depth of field
// ---------------------------------------------------------------------------

/// Default circle of confusion: sensor diagonal / 1500 (the "Zeiss formula"
/// used for cine lenses; 0.029 mm on full frame, 0.023 mm on Super 35).
pub fn circle_of_confusion_mm(sensor_width_mm: f64, sensor_height_mm: f64) -> f64 {
    (sensor_width_mm * sensor_width_mm + sensor_height_mm * sensor_height_mm).sqrt() / 1500.0
}

/// Circle of confusion for a camera's sensor (see [`circle_of_confusion_mm`]).
pub fn coc_for(p: &CameraParams) -> f64 {
    circle_of_confusion_mm(p.sensor_width_mm, p.sensor_height_mm)
}

/// Hyperfocal distance in metres: `f² / (N·c) + f`.
pub fn hyperfocal_m(focal_mm: f64, f_stop: f64, coc_mm: f64) -> f64 {
    (focal_mm * focal_mm / (f_stop * coc_mm) + focal_mm) / 1000.0
}

/// Near limit of acceptable sharpness in metres for a lens focused at `focus_m`.
pub fn dof_near_m(focal_mm: f64, f_stop: f64, coc_mm: f64, focus_m: f64) -> f64 {
    let h = hyperfocal_m(focal_mm, f_stop, coc_mm) * 1000.0;
    let s = focus_m * 1000.0;
    let f = focal_mm;
    (s * (h - f) / (h + s - 2.0 * f)) / 1000.0
}

/// Far limit of acceptable sharpness in metres. Returns `f64::INFINITY` when
/// the focus distance is at or beyond the hyperfocal distance.
pub fn dof_far_m(focal_mm: f64, f_stop: f64, coc_mm: f64, focus_m: f64) -> f64 {
    let h = hyperfocal_m(focal_mm, f_stop, coc_mm) * 1000.0;
    let s = focus_m * 1000.0;
    let f = focal_mm;
    if s >= h {
        f64::INFINITY
    } else {
        (s * (h - f) / (h - s)) / 1000.0
    }
}

/// Total depth of field in metres (`INFINITY` beyond the hyperfocal distance).
pub fn total_dof_m(focal_mm: f64, f_stop: f64, coc_mm: f64, focus_m: f64) -> f64 {
    dof_far_m(focal_mm, f_stop, coc_mm, focus_m) - dof_near_m(focal_mm, f_stop, coc_mm, focus_m)
}

/// Depth of field for a camera, `(near_m, far_m)`, using the default CoC.
pub fn dof_for(p: &CameraParams) -> (f64, f64) {
    let c = coc_for(p);
    (
        dof_near_m(p.focal_length_mm, p.f_stop, c, p.focus_distance_m),
        dof_far_m(p.focal_length_mm, p.f_stop, c, p.focus_distance_m),
    )
}

/// Diameter (mm on the sensor) of the blur circle of a point at
/// `distance_m` when the lens is focused at `focus_m` (thin lens).
pub fn blur_diameter_mm(focal_mm: f64, f_stop: f64, focus_m: f64, distance_m: f64) -> f64 {
    let f = focal_mm;
    let s = focus_m * 1000.0;
    let d = distance_m * 1000.0;
    if d <= 0.0 || s <= f {
        return 0.0;
    }
    (f * f / f_stop) * ((d - s).abs() / (d * (s - f)))
}

/// Blur circle diameter in output pixels for a point at `distance_m`, using
/// the camera's focus distance, aperture and horizontal resolution.
pub fn blur_diameter_px(p: &CameraParams, distance_m: f64) -> f64 {
    let mm = blur_diameter_mm(p.focal_length_mm, p.f_stop, p.focus_distance_m, distance_m);
    mm * (p.resolution[0] as f64 / p.sensor_width_mm)
}

// ---------------------------------------------------------------------------
// Exposure
// ---------------------------------------------------------------------------

/// T-stop from f-stop and lens transmission (`0 < transmission ≤ 1`):
/// `T = N / √t`.
pub fn f_stop_to_t_stop(f_stop: f64, transmission: f64) -> f64 {
    f_stop / transmission.max(1e-9).sqrt()
}

/// f-stop from T-stop and transmission: `N = T · √t`.
pub fn t_stop_to_f_stop(t_stop: f64, transmission: f64) -> f64 {
    t_stop * transmission.max(0.0).sqrt()
}

/// Exposure time in seconds for a shutter angle at a frame rate:
/// `angle / 360 / fps` (180° at 24 fps = 1/48 s).
pub fn shutter_angle_to_seconds(shutter_angle_deg: f64, fps: f64) -> f64 {
    shutter_angle_deg / 360.0 / fps
}

/// Shutter angle for an exposure time at a frame rate.
pub fn seconds_to_shutter_angle(seconds: f64, fps: f64) -> f64 {
    seconds * fps * 360.0
}

/// Exposure value at ISO 100 (`EV100`) that the given settings expose
/// correctly: `log2(N² / t) − log2(ISO / 100)`.
///
/// Example: ISO 800, f/2.8, 180° @ 24 fps → EV100 ≈ 5.56 (a dim interior).
pub fn ev(iso: f64, f_stop: f64, shutter_angle_deg: f64, fps: f64) -> f64 {
    let t = shutter_angle_to_seconds(shutter_angle_deg, fps);
    (f_stop * f_stop / t).log2() - (iso / 100.0).log2()
}

/// Exposure value of a camera's current settings (see [`ev`]).
pub fn ev_for(p: &CameraParams) -> f64 {
    ev(p.iso, p.f_stop, p.shutter_angle_deg, p.frame_rate)
}

// ---------------------------------------------------------------------------
// Projection
// ---------------------------------------------------------------------------

/// Validate that camera parameters are physically usable for projection.
fn check_params(p: &CameraParams) -> Result<()> {
    let positive = |x: f64| x.is_finite() && x > 0.0;
    if !positive(p.focal_length_mm) {
        return Err(Error::Optics("focal length must be > 0".into()));
    }
    if !positive(p.sensor_width_mm) || !positive(p.sensor_height_mm) {
        return Err(Error::Optics("sensor dimensions must be > 0".into()));
    }
    if !positive(p.clip[0]) || !positive(p.clip[1] - p.clip[0]) {
        return Err(Error::Optics(
            "clip planes must satisfy 0 < near < far".into(),
        ));
    }
    Ok(())
}

/// OpenGL-style perspective projection (right-handed, NDC z ∈ [-1, 1]) from
/// the sensor size, focal length and clip planes.
pub fn projection_matrix(p: &CameraParams) -> DMat4 {
    projection_matrix_anamorphic(p, AnamorphicSqueeze::Spherical)
}

/// [`projection_matrix`] with the horizontal aperture widened by an
/// anamorphic squeeze factor.
pub fn projection_matrix_anamorphic(p: &CameraParams, squeeze: AnamorphicSqueeze) -> DMat4 {
    let fovy = vertical_fov(p).to_radians();
    let aspect = delivered_aspect(p, squeeze);
    DMat4::perspective_rh_gl(fovy, aspect, p.clip[0], p.clip[1])
}

/// View matrix (world → camera) from a camera transform.
pub fn view_matrix(t: &Transform) -> DMat4 {
    t.to_mat4().inverse()
}

/// Camera transform that places the eye at `eye` looking at `target`.
/// `up` is the approximate up vector (use `[0,1,0]` unless the camera rolls).
pub fn look_at(eye: [f64; 3], target: [f64; 3], up: [f64; 3]) -> Transform {
    let eye = DVec3::from_array(eye);
    let target = DVec3::from_array(target);
    let mut up = DVec3::from_array(up);
    let mut forward = target - eye;
    if forward.length_squared() < 1e-18 {
        forward = -DVec3::Z;
    }
    let forward = forward.normalize();
    if forward.cross(up).length_squared() < 1e-12 {
        // Looking straight up/down: pick any perpendicular up.
        up = if forward.y.abs() > 0.9 {
            DVec3::Z
        } else {
            DVec3::Y
        };
    }
    let right = forward.cross(up).normalize();
    let true_up = right.cross(forward);
    // Camera looks down -Z, so the local Z axis is -forward.
    let rot = glam::DMat3::from_cols(right, true_up, -forward);
    let q = DQuat::from_mat3(&rot).normalize();
    Transform {
        translate: eye.to_array(),
        rotate: q.to_array(),
        scale: [1.0; 3],
    }
}

/// World-space forward direction (-Z) of a camera transform.
pub fn forward_of(t: &Transform) -> DVec3 {
    DQuat::from_array(t.rotate).normalize() * -DVec3::Z
}

/// World-space up direction (+Y) of a camera transform.
pub fn up_of(t: &Transform) -> DVec3 {
    DQuat::from_array(t.rotate).normalize() * DVec3::Y
}

/// World-space right direction (+X) of a camera transform.
pub fn right_of(t: &Transform) -> DVec3 {
    DQuat::from_array(t.rotate).normalize() * DVec3::X
}

/// Project a world point to OpenGL NDC (`x, y, z ∈ [-1,1]` when inside the
/// frustum). Returns `None` for points at or behind the camera plane.
pub fn world_to_ndc(p: &CameraParams, cam: &Transform, world: [f64; 3]) -> Option<DVec3> {
    let clip =
        projection_matrix(p) * view_matrix(cam) * DVec4::new(world[0], world[1], world[2], 1.0);
    if clip.w <= 1e-12 {
        return None;
    }
    Some(DVec3::new(
        clip.x / clip.w,
        clip.y / clip.w,
        clip.z / clip.w,
    ))
}

/// Project a world point to frame coordinates (`0..1`, origin top-left,
/// `y` down). Points outside the frame yield values outside `0..1`; points
/// behind the camera yield `None`.
pub fn project_point(p: &CameraParams, cam: &Transform, world: [f64; 3]) -> Option<[f64; 2]> {
    let ndc = world_to_ndc(p, cam, world)?;
    Some(ndc_to_frame([ndc.x, ndc.y]))
}

/// NDC (`y` up) → frame coordinates (`y` down).
pub fn ndc_to_frame(ndc: [f64; 2]) -> [f64; 2] {
    [(ndc[0] + 1.0) * 0.5, (1.0 - ndc[1]) * 0.5]
}

/// Frame coordinates (`y` down) → NDC (`y` up).
pub fn frame_to_ndc(frame: [f64; 2]) -> [f64; 2] {
    [frame[0] * 2.0 - 1.0, 1.0 - frame[1] * 2.0]
}

/// The world-space point that a frame coordinate maps to on the plane
/// `depth_m` in front of the camera (perpendicular to the view axis).
pub fn unproject_frame_point(
    p: &CameraParams,
    cam: &Transform,
    frame: [f64; 2],
    depth_m: f64,
) -> [f64; 3] {
    let ndc = frame_to_ndc(frame);
    let half_w = depth_m * p.sensor_width_mm / (2.0 * p.focal_length_mm);
    let half_h = depth_m * p.sensor_height_mm / (2.0 * p.focal_length_mm);
    let local = DVec4::new(ndc[0] * half_w, ndc[1] * half_h, -depth_m, 1.0);
    (cam.to_mat4() * local).truncate().to_array()
}

/// Inverse of [`unproject_frame_point`]: frame coordinate and depth (metres
/// along the view axis) of a world point. `None` if behind the camera.
pub fn world_to_frame_depth(
    p: &CameraParams,
    cam: &Transform,
    world: [f64; 3],
) -> Option<([f64; 2], f64)> {
    let local = view_matrix(cam) * DVec4::new(world[0], world[1], world[2], 1.0);
    let depth = -local.z;
    if depth <= 1e-12 {
        return None;
    }
    let half_w = depth * p.sensor_width_mm / (2.0 * p.focal_length_mm);
    let half_h = depth * p.sensor_height_mm / (2.0 * p.focal_length_mm);
    Some((ndc_to_frame([local.x / half_w, local.y / half_h]), depth))
}

/// A camera frustum in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum {
    /// Near-plane corners: bottom-left, bottom-right, top-right, top-left.
    pub near: [DVec3; 4],
    /// Far-plane corners in the same order.
    pub far: [DVec3; 4],
    /// Planes as `(nx, ny, nz, d)` with inward-pointing normals, in the order
    /// left, right, bottom, top, near, far. A point is inside when
    /// `n·p + d ≥ 0` for every plane.
    pub planes: [DVec4; 6],
}

impl Frustum {
    /// Build the frustum from a camera transform and parameters (clip planes
    /// come from `params.clip`). Fails on non-physical parameters.
    pub fn from_camera(cam: &Transform, p: &CameraParams) -> Result<Frustum> {
        check_params(p)?;
        Ok(Self::from_camera_squeezed(
            cam,
            p,
            AnamorphicSqueeze::Spherical,
        ))
    }

    /// Like [`Frustum::from_camera`] with an anamorphic squeeze; parameters
    /// are assumed valid.
    pub fn from_camera_squeezed(
        cam: &Transform,
        p: &CameraParams,
        squeeze: AnamorphicSqueeze,
    ) -> Frustum {
        let m = cam.to_mat4();
        let w = effective_sensor_width_mm(p, squeeze);
        let corners = |depth: f64| -> [DVec3; 4] {
            let hw = depth * w / (2.0 * p.focal_length_mm);
            let hh = depth * p.sensor_height_mm / (2.0 * p.focal_length_mm);
            let local = [
                DVec3::new(-hw, -hh, -depth),
                DVec3::new(hw, -hh, -depth),
                DVec3::new(hw, hh, -depth),
                DVec3::new(-hw, hh, -depth),
            ];
            local.map(|l| m.transform_point3(l))
        };
        let near = corners(p.clip[0]);
        let far = corners(p.clip[1]);
        let eye = m.transform_point3(DVec3::ZERO);
        let plane = |a: DVec3, b: DVec3, c: DVec3| -> DVec4 {
            let n = (b - a).cross(c - a).normalize();
            DVec4::new(n.x, n.y, n.z, -n.dot(a))
        };
        // Inward normals: wind so that the cross product points into the volume.
        let left = plane(eye, near[0], near[3]);
        let right = plane(eye, near[2], near[1]);
        let bottom = plane(eye, near[1], near[0]);
        let top = plane(eye, near[3], near[2]);
        let near_p = plane(near[0], near[3], near[2]);
        let far_p = plane(far[0], far[1], far[2]);
        Frustum {
            near,
            far,
            planes: [left, right, bottom, top, near_p, far_p],
        }
    }

    /// Whether a world point lies inside (or on the boundary of) the frustum.
    pub fn contains_point(&self, world: [f64; 3]) -> bool {
        let p = DVec4::new(world[0], world[1], world[2], 1.0);
        self.planes.iter().all(|pl| pl.dot(p) >= -1e-9)
    }

    /// Whether an axis-aligned box intersects the frustum (conservative).
    pub fn intersects_aabb(&self, min: [f64; 3], max: [f64; 3]) -> bool {
        self.planes.iter().all(|pl| {
            // Pick the box corner most along the normal.
            let px = if pl.x >= 0.0 { max[0] } else { min[0] };
            let py = if pl.y >= 0.0 { max[1] } else { min[1] };
            let pz = if pl.z >= 0.0 { max[2] } else { min[2] };
            pl.dot(DVec4::new(px, py, pz, 1.0)) >= -1e-9
        })
    }

    /// The 8 corners as a flat list (near then far) for line drawing, with
    /// the 12 edge index pairs.
    pub fn wire(&self) -> ([DVec3; 8], [[u32; 2]; 12]) {
        let corners = [
            self.near[0],
            self.near[1],
            self.near[2],
            self.near[3],
            self.far[0],
            self.far[1],
            self.far[2],
            self.far[3],
        ];
        let edges = [
            [0, 1],
            [1, 2],
            [2, 3],
            [3, 0],
            [4, 5],
            [5, 6],
            [6, 7],
            [7, 4],
            [0, 4],
            [1, 5],
            [2, 6],
            [3, 7],
        ];
        (corners, edges)
    }
}

/// Width and height (metres) of the frame at `depth_m` in front of the camera.
pub fn frame_size_at(p: &CameraParams, depth_m: f64) -> [f64; 2] {
    [
        depth_m * p.sensor_width_mm / p.focal_length_mm,
        depth_m * p.sensor_height_mm / p.focal_length_mm,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ff35() -> CameraParams {
        let mut p = CameraParams::default();
        SensorPreset::FullFrame.apply(&mut p);
        p.focal_length_mm = 35.0;
        p.f_stop = 2.8;
        p.focus_distance_m = 3.0;
        p
    }

    #[test]
    fn presets_have_expected_sizes() {
        let s = SensorPreset::ArriAlexa35.sensor();
        assert_eq!((s.width_mm, s.height_mm), (27.99, 19.22));
        assert_eq!(SensorPreset::ALL.len(), 10);
        assert!((SensorPreset::FullFrame.sensor().crop_factor() - 1.0).abs() < 1e-12);
        assert!((SensorPreset::Super35.sensor().crop_factor() - 1.39).abs() < 0.01);
    }

    #[test]
    fn fov_and_focal_are_inverse() {
        let f = focal_for_fov(36.0, fov_deg(36.0, 50.0));
        assert!((f - 50.0).abs() < 1e-9);
        let p = ff35();
        assert!((horizontal_fov(&p) - 54.43).abs() < 0.01);
        assert!((vertical_fov(&p) - 37.85).abs() < 0.01);
        assert!(diagonal_fov(&p) > horizontal_fov(&p));
    }

    #[test]
    fn anamorphic_widens_horizontal_only() {
        let p = ff35();
        let h2 = horizontal_fov_anamorphic(&p, AnamorphicSqueeze::X2);
        assert!(h2 > horizontal_fov(&p));
        assert!((h2 - fov_deg(72.0, 35.0)).abs() < 1e-9);
        assert!((delivered_aspect(&p, AnamorphicSqueeze::X2) - 3.0).abs() < 1e-12);
    }

    #[test]
    fn exposure_math() {
        assert!((shutter_angle_to_seconds(180.0, 24.0) - 1.0 / 48.0).abs() < 1e-12);
        assert!((seconds_to_shutter_angle(1.0 / 48.0, 24.0) - 180.0).abs() < 1e-9);
        assert!((ev(100.0, 2.8, 180.0, 24.0) - 8.556).abs() < 0.01);
        assert!((ev(800.0, 2.8, 180.0, 24.0) - 5.556).abs() < 0.01);
        assert!((f_stop_to_t_stop(2.0, 0.81) - 2.222).abs() < 0.01);
        assert!((t_stop_to_f_stop(f_stop_to_t_stop(2.8, 0.7), 0.7) - 2.8).abs() < 1e-12);
    }

    #[test]
    fn blur_is_zero_at_focus_and_grows_away() {
        let p = ff35();
        assert!(blur_diameter_px(&p, 3.0).abs() < 1e-12);
        assert!(blur_diameter_px(&p, 1.0) > blur_diameter_px(&p, 2.0));
        assert!(blur_diameter_px(&p, 30.0) > blur_diameter_px(&p, 6.0));
    }

    #[test]
    fn look_at_faces_target() {
        let t = look_at([0.0, 1.0, 5.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]);
        let f = forward_of(&t);
        assert!((f - DVec3::new(0.0, 0.0, -1.0)).length() < 1e-9);
        let t = look_at([3.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!((forward_of(&t) - DVec3::new(-1.0, 0.0, 0.0)).length() < 1e-9);
        assert!((up_of(&t) - DVec3::Y).length() < 1e-9);
    }

    #[test]
    fn frame_unproject_round_trip() {
        let p = ff35();
        let cam = look_at([1.0, 2.0, 3.0], [0.0, 1.0, -2.0], [0.0, 1.0, 0.0]);
        for (u, v) in [(0.0, 0.0), (0.5, 0.5), (1.0, 1.0), (0.25, 0.8)] {
            let w = unproject_frame_point(&p, &cam, [u, v], 4.0);
            let (f, d) = world_to_frame_depth(&p, &cam, w).unwrap();
            assert!((f[0] - u).abs() < 1e-9 && (f[1] - v).abs() < 1e-9);
            assert!((d - 4.0).abs() < 1e-9);
            let f2 = project_point(&p, &cam, w).unwrap();
            assert!((f2[0] - u).abs() < 1e-9 && (f2[1] - v).abs() < 1e-9);
        }
    }

    #[test]
    fn frustum_contains_and_rejects() {
        let p = ff35();
        let cam = Transform::default();
        let fr = Frustum::from_camera(&cam, &p).unwrap();
        assert!(fr.contains_point([0.0, 0.0, -5.0]));
        assert!(!fr.contains_point([0.0, 0.0, 5.0]));
        assert!(!fr.contains_point([100.0, 0.0, -5.0]));
        assert!(!fr.contains_point([0.0, 0.0, -0.001]));
        assert!(fr.intersects_aabb([-1.0, -1.0, -6.0], [1.0, 1.0, -4.0]));
        assert!(!fr.intersects_aabb([-1.0, -1.0, 4.0], [1.0, 1.0, 6.0]));
        let (_, edges) = fr.wire();
        assert_eq!(edges.len(), 12);
    }

    #[test]
    fn rejects_bad_params() {
        let mut p = ff35();
        p.focal_length_mm = 0.0;
        assert!(Frustum::from_camera(&Transform::default(), &p).is_err());
    }
}
