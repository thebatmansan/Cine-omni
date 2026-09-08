//! Camera rigs: crane forward kinematics, dolly tracks, keyframe animation.
//!
//! Every rig produces plain [`Transform`]s in the crate's Y-up, camera
//! looks down -Z convention (see [`crate::optics`]).

use glam::{DMat4, DQuat, DVec3};
use serde::{Deserialize, Serialize};
use ucg_schema::{CameraParams, Transform};

use crate::error::{Error, Result};
use crate::optics::look_at;

// ---------------------------------------------------------------------------
// Interpolation
// ---------------------------------------------------------------------------

/// Types that can be blended between two keyframes.
pub trait Interpolate: Clone {
    /// Blend `self` (t = 0) towards `other` (t = 1).
    fn lerp(&self, other: &Self, t: f64) -> Self;
}

impl Interpolate for f64 {
    fn lerp(&self, other: &Self, t: f64) -> Self {
        self + (other - self) * t
    }
}

impl Interpolate for [f64; 3] {
    fn lerp(&self, other: &Self, t: f64) -> Self {
        [
            self[0].lerp(&other[0], t),
            self[1].lerp(&other[1], t),
            self[2].lerp(&other[2], t),
        ]
    }
}

impl Interpolate for Transform {
    /// Linear translate/scale, spherical-linear rotation (shortest arc).
    fn lerp(&self, other: &Self, t: f64) -> Self {
        let qa = DQuat::from_array(self.rotate).normalize();
        let qb = DQuat::from_array(other.rotate).normalize();
        // `slerp` in glam already takes the shortest path.
        let q = qa.slerp(qb, t).normalize();
        Transform {
            translate: self.translate.lerp(&other.translate, t),
            rotate: q.to_array(),
            scale: self.scale.lerp(&other.scale, t),
        }
    }
}

impl Interpolate for CameraParams {
    /// Blends every continuous parameter (focal pulls, focus pulls, iris
    /// ramps...). Resolution snaps at `t = 0.5`.
    fn lerp(&self, other: &Self, t: f64) -> Self {
        CameraParams {
            sensor_width_mm: self.sensor_width_mm.lerp(&other.sensor_width_mm, t),
            sensor_height_mm: self.sensor_height_mm.lerp(&other.sensor_height_mm, t),
            focal_length_mm: self.focal_length_mm.lerp(&other.focal_length_mm, t),
            f_stop: self.f_stop.lerp(&other.f_stop, t),
            focus_distance_m: self.focus_distance_m.lerp(&other.focus_distance_m, t),
            shutter_angle_deg: self.shutter_angle_deg.lerp(&other.shutter_angle_deg, t),
            iso: self.iso.lerp(&other.iso, t),
            white_balance_k: self.white_balance_k.lerp(&other.white_balance_k, t),
            frame_rate: self.frame_rate.lerp(&other.frame_rate, t),
            resolution: if t < 0.5 {
                self.resolution
            } else {
                other.resolution
            },
            clip: [
                self.clip[0].lerp(&other.clip[0], t),
                self.clip[1].lerp(&other.clip[1], t),
            ],
        }
    }
}

/// Easing applied to the segment that starts at a keyframe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    /// Constant speed.
    #[default]
    Linear,
    /// Smoothstep: slow start and end (the classic camera-move feel).
    EaseInOut,
    /// Slow start, full speed at the end.
    EaseIn,
    /// Full speed at the start, slow end.
    EaseOut,
    /// Hold the key's value until the next key (stepped).
    Hold,
}

impl Ease {
    /// Remap a linear `0..1` parameter.
    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Ease::Linear => t,
            Ease::EaseInOut => t * t * (3.0 - 2.0 * t),
            Ease::EaseIn => t * t,
            Ease::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Ease::Hold => 0.0,
        }
    }
}

/// A value at a point in time. The `ease` governs the interpolation from
/// this key to the next one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe<T> {
    /// Time in seconds.
    pub time_s: f64,
    /// The keyed value.
    pub value: T,
    /// Easing towards the following key.
    pub ease: Ease,
}

impl<T> Keyframe<T> {
    /// A linear keyframe.
    pub fn new(time_s: f64, value: T) -> Self {
        Keyframe {
            time_s,
            value,
            ease: Ease::Linear,
        }
    }
    /// A keyframe with an explicit easing.
    pub fn with_ease(time_s: f64, value: T, ease: Ease) -> Self {
        Keyframe {
            time_s,
            value,
            ease,
        }
    }
}

/// A time-sorted list of keyframes of one value type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track<T> {
    /// Keys, kept sorted by time by [`Track::insert`].
    pub keys: Vec<Keyframe<T>>,
}

impl<T> Default for Track<T> {
    fn default() -> Self {
        Track { keys: Vec::new() }
    }
}

impl<T: Interpolate> Track<T> {
    /// An empty track.
    pub fn new() -> Self {
        Self::default()
    }
    /// Insert a key keeping time order (replaces a key at the same time).
    pub fn insert(&mut self, key: Keyframe<T>) {
        match self.keys.binary_search_by(|k| {
            k.time_s
                .partial_cmp(&key.time_s)
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            Ok(i) => self.keys[i] = key,
            Err(i) => self.keys.insert(i, key),
        }
    }
    /// Whether the track has no keys.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
    /// Time of the last key (0 when empty).
    pub fn duration_s(&self) -> f64 {
        self.keys.last().map(|k| k.time_s).unwrap_or(0.0)
    }
    /// Value at `time_s`: clamped before the first / after the last key,
    /// eased interpolation in between. `None` for an empty track.
    pub fn sample(&self, time_s: f64) -> Option<T> {
        let first = self.keys.first()?;
        if self.keys.len() == 1 || time_s <= first.time_s {
            return Some(first.value.clone());
        }
        let last = self.keys.last()?;
        if time_s >= last.time_s {
            return Some(last.value.clone());
        }
        let i = self.keys.partition_point(|k| k.time_s <= time_s);
        let a = &self.keys[i - 1];
        let b = &self.keys[i];
        let span = b.time_s - a.time_s;
        let t = if span <= 0.0 {
            1.0
        } else {
            (time_s - a.time_s) / span
        };
        Some(a.value.lerp(&b.value, a.ease.apply(t)))
    }
}

/// Keyframed camera animation: a transform track plus a camera-parameter
/// track (focal / focus / iris pulls). Either track may be empty.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Animation {
    /// Camera placement over time.
    pub transform: Track<Transform>,
    /// Lens / exposure parameters over time.
    pub camera: Track<CameraParams>,
}

/// The state of an [`Animation`] at one instant.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationSample {
    /// Interpolated camera transform (`None` if the track is empty).
    pub transform: Option<Transform>,
    /// Interpolated camera parameters (`None` if the track is empty).
    pub camera: Option<CameraParams>,
}

impl Animation {
    /// An animation with no keys.
    pub fn new() -> Self {
        Self::default()
    }
    /// Add a transform key.
    pub fn key_transform(&mut self, time_s: f64, value: Transform, ease: Ease) -> &mut Self {
        self.transform
            .insert(Keyframe::with_ease(time_s, value, ease));
        self
    }
    /// Add a camera-parameter key.
    pub fn key_camera(&mut self, time_s: f64, value: CameraParams, ease: Ease) -> &mut Self {
        self.camera.insert(Keyframe::with_ease(time_s, value, ease));
        self
    }
    /// Length of the longest track in seconds.
    pub fn duration_s(&self) -> f64 {
        self.transform.duration_s().max(self.camera.duration_s())
    }
    /// Evaluate both tracks at `time_s`.
    pub fn sample(&self, time_s: f64) -> AnimationSample {
        AnimationSample {
            transform: self.transform.sample(time_s),
            camera: self.camera.sample(time_s),
        }
    }
}

// ---------------------------------------------------------------------------
// Crane
// ---------------------------------------------------------------------------

/// A camera crane / jib. Forward kinematics chain, all angles in degrees:
///
/// ```text
/// base → swing (yaw about Y) → boom (pitch about X, positive raises the arm)
///      → arm (extends along the local -Z, i.e. "forward")
///      → head pan (Y) → tilt (X) → roll (Z) → camera
/// ```
///
/// With every angle at zero the camera sits at `base + (0, 0, -arm_length)`
/// looking down -Z.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CraneRig {
    /// Pivot of the crane base in world space (metres).
    pub base: [f64; 3],
    /// Arm length from the pivot to the head (metres).
    pub arm_length_m: f64,
    /// Boom elevation angle (degrees, positive raises the head).
    pub boom_deg: f64,
    /// Swing about the vertical axis (degrees, positive turns left).
    pub swing_deg: f64,
    /// Remote head pan (degrees).
    pub pan_deg: f64,
    /// Remote head tilt (degrees, positive looks up).
    pub tilt_deg: f64,
    /// Remote head roll (degrees, positive is counter-clockwise).
    pub roll_deg: f64,
}

impl Default for CraneRig {
    fn default() -> Self {
        CraneRig {
            base: [0.0, 1.2, 0.0],
            arm_length_m: 4.0,
            boom_deg: 0.0,
            swing_deg: 0.0,
            pan_deg: 0.0,
            tilt_deg: 0.0,
            roll_deg: 0.0,
        }
    }
}

impl CraneRig {
    /// Matrix from the base pivot to the arm tip (before the head).
    fn arm_matrix(&self) -> DMat4 {
        DMat4::from_translation(DVec3::from_array(self.base))
            * DMat4::from_rotation_y(self.swing_deg.to_radians())
            * DMat4::from_rotation_x(self.boom_deg.to_radians())
            * DMat4::from_translation(DVec3::new(0.0, 0.0, -self.arm_length_m))
    }

    /// World position of the arm tip / head pivot.
    pub fn arm_tip(&self) -> [f64; 3] {
        self.arm_matrix().transform_point3(DVec3::ZERO).to_array()
    }

    /// Full forward kinematics: the camera transform.
    pub fn camera_transform(&self) -> Transform {
        let m = self.arm_matrix()
            * DMat4::from_rotation_y(self.pan_deg.to_radians())
            * DMat4::from_rotation_x(self.tilt_deg.to_radians())
            * DMat4::from_rotation_z(self.roll_deg.to_radians());
        Transform::from_mat4(&m)
    }

    /// Solve the head pan/tilt so the camera looks at `target` (roll kept).
    pub fn aim_at(&mut self, target: [f64; 3]) {
        let tip = DVec3::from_array(self.arm_tip());
        // Direction in the arm's local frame (undo swing/boom).
        let arm_rot = DMat4::from_rotation_y(self.swing_deg.to_radians())
            * DMat4::from_rotation_x(self.boom_deg.to_radians());
        let d = arm_rot
            .inverse()
            .transform_vector3(DVec3::from_array(target) - tip);
        if d.length_squared() < 1e-18 {
            return;
        }
        // Camera forward is -Z; pan about Y then tilt about X.
        self.pan_deg = (-d.x).atan2(-d.z).to_degrees();
        self.tilt_deg = d.y.atan2((d.x * d.x + d.z * d.z).sqrt()).to_degrees();
    }

    /// Interpolate all angles and the base linearly.
    pub fn lerp(&self, other: &CraneRig, t: f64) -> CraneRig {
        CraneRig {
            base: self.base.lerp(&other.base, t),
            arm_length_m: self.arm_length_m.lerp(&other.arm_length_m, t),
            boom_deg: self.boom_deg.lerp(&other.boom_deg, t),
            swing_deg: self.swing_deg.lerp(&other.swing_deg, t),
            pan_deg: self.pan_deg.lerp(&other.pan_deg, t),
            tilt_deg: self.tilt_deg.lerp(&other.tilt_deg, t),
            roll_deg: self.roll_deg.lerp(&other.roll_deg, t),
        }
    }
}

// ---------------------------------------------------------------------------
// Dolly
// ---------------------------------------------------------------------------

/// A dolly track: a Catmull-Rom spline through `points` with the camera
/// mounted at `camera_offset` (in track-local space: X right of travel,
/// Y up, Z opposite to travel). The camera faces along the direction of
/// travel unless `look_at` is set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DollyTrack {
    /// Control points in world space (at least two).
    pub points: Vec<[f64; 3]>,
    /// Camera offset from the track (metres, track-local frame).
    pub camera_offset: [f64; 3],
    /// Optional fixed target the camera keeps framing.
    pub look_at: Option<[f64; 3]>,
}

impl DollyTrack {
    /// A straight or curved track through the given points.
    pub fn new(points: Vec<[f64; 3]>) -> Self {
        DollyTrack {
            points,
            camera_offset: [0.0, 1.2, 0.0],
            look_at: None,
        }
    }

    /// Validate the track (needs ≥ 2 points).
    pub fn validate(&self) -> Result<()> {
        if self.points.len() < 2 {
            return Err(Error::Rig("a dolly track needs at least two points".into()));
        }
        Ok(())
    }

    fn control(&self, i: isize) -> DVec3 {
        let n = self.points.len() as isize;
        let i = i.clamp(0, n - 1) as usize;
        DVec3::from_array(self.points[i])
    }

    /// Map `t ∈ 0..1` over the whole track to (segment, local u).
    fn locate(&self, t: f64) -> (isize, f64) {
        let segs = (self.points.len().max(2) - 1) as f64;
        let s = (t.clamp(0.0, 1.0) * segs).min(segs - 1e-12);
        let seg = s.floor();
        (seg as isize, s - seg)
    }

    /// Position along the track (uniform Catmull-Rom, clamped ends).
    pub fn position(&self, t: f64) -> [f64; 3] {
        if self.points.is_empty() {
            return [0.0; 3];
        }
        if self.points.len() == 1 {
            return self.points[0];
        }
        let (seg, u) = self.locate(t);
        let (p0, p1, p2, p3) = (
            self.control(seg - 1),
            self.control(seg),
            self.control(seg + 1),
            self.control(seg + 2),
        );
        catmull_rom(p0, p1, p2, p3, u).to_array()
    }

    /// Unit tangent (direction of travel) along the track.
    pub fn tangent(&self, t: f64) -> [f64; 3] {
        if self.points.len() < 2 {
            return [0.0, 0.0, -1.0];
        }
        let (seg, u) = self.locate(t);
        let (p0, p1, p2, p3) = (
            self.control(seg - 1),
            self.control(seg),
            self.control(seg + 1),
            self.control(seg + 2),
        );
        let d = catmull_rom_derivative(p0, p1, p2, p3, u);
        let d = if d.length_squared() < 1e-18 {
            p2 - p1
        } else {
            d
        };
        if d.length_squared() < 1e-18 {
            [0.0, 0.0, -1.0]
        } else {
            d.normalize().to_array()
        }
    }

    /// Camera transform at `t ∈ 0..1` along the track.
    pub fn at(&self, t: f64) -> Transform {
        let pos = DVec3::from_array(self.position(t));
        let fwd = DVec3::from_array(self.tangent(t));
        // Track-local frame: forward = travel direction, right = fwd × up.
        let mut up = DVec3::Y;
        if fwd.cross(up).length_squared() < 1e-12 {
            up = DVec3::Z;
        }
        let right = fwd.cross(up).normalize();
        let true_up = right.cross(fwd).normalize();
        let off = self.camera_offset;
        let eye = pos + right * off[0] + true_up * off[1] - fwd * off[2];
        match self.look_at {
            Some(target) => look_at(eye.to_array(), target, [0.0, 1.0, 0.0]),
            None => look_at(eye.to_array(), (eye + fwd).to_array(), [0.0, 1.0, 0.0]),
        }
    }

    /// Approximate arc length by sampling.
    pub fn length_m(&self, samples: usize) -> f64 {
        let n = samples.max(2);
        let mut len = 0.0;
        let mut prev = DVec3::from_array(self.position(0.0));
        for i in 1..n {
            let p = DVec3::from_array(self.position(i as f64 / (n - 1) as f64));
            len += (p - prev).length();
            prev = p;
        }
        len
    }
}

/// Uniform Catmull-Rom interpolation between `p1` and `p2`.
pub fn catmull_rom(p0: DVec3, p1: DVec3, p2: DVec3, p3: DVec3, u: f64) -> DVec3 {
    let u2 = u * u;
    let u3 = u2 * u;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * u
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * u2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * u3)
}

/// Derivative of [`catmull_rom`] with respect to `u`.
pub fn catmull_rom_derivative(p0: DVec3, p1: DVec3, p2: DVec3, p3: DVec3, u: f64) -> DVec3 {
    let u2 = u * u;
    0.5 * ((-p0 + p2)
        + 2.0 * (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * u
        + 3.0 * (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * u2)
}

// ---------------------------------------------------------------------------
// Rig
// ---------------------------------------------------------------------------

/// Any camera support, evaluated on a normalized `0..1` parameter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Rig {
    /// A locked-off camera.
    Static(Transform),
    /// A crane held in one pose.
    Crane(CraneRig),
    /// A crane move blended between two poses.
    CraneMove {
        /// Pose at `t = 0`.
        from: CraneRig,
        /// Pose at `t = 1`.
        to: CraneRig,
        /// Easing of the move.
        ease: Ease,
    },
    /// A dolly on a track.
    Dolly(DollyTrack),
    /// A free keyframed move (`t` maps onto the animation duration).
    Animated(Animation),
}

impl Rig {
    /// Camera transform at normalized parameter `t ∈ 0..1`.
    pub fn at(&self, t: f64) -> Transform {
        match self {
            Rig::Static(x) => *x,
            Rig::Crane(c) => c.camera_transform(),
            Rig::CraneMove { from, to, ease } => from.lerp(to, ease.apply(t)).camera_transform(),
            Rig::Dolly(d) => d.at(t),
            Rig::Animated(a) => a
                .sample(t.clamp(0.0, 1.0) * a.duration_s())
                .transform
                .unwrap_or_default(),
        }
    }

    /// `samples` transforms evenly spaced over the move, for drawing the
    /// camera path in the viewport. Returns at least one transform.
    pub fn path(&self, samples: usize) -> Vec<Transform> {
        let n = samples.max(1);
        if n == 1 {
            return vec![self.at(0.0)];
        }
        (0..n).map(|i| self.at(i as f64 / (n - 1) as f64)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optics::forward_of;

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9)
    }

    #[test]
    fn crane_rest_pose() {
        let c = CraneRig {
            base: [1.0, 2.0, 3.0],
            arm_length_m: 4.0,
            ..Default::default()
        };
        let t = c.camera_transform();
        assert!(close(t.translate, [1.0, 2.0, -1.0]));
        assert!(close(forward_of(&t).to_array(), [0.0, 0.0, -1.0]));
    }

    #[test]
    fn crane_boom_raises_and_swing_turns() {
        let c = CraneRig {
            base: [0.0; 3],
            arm_length_m: 2.0,
            boom_deg: 90.0,
            ..Default::default()
        };
        assert!(close(c.arm_tip(), [0.0, 2.0, 0.0]));
        let c = CraneRig {
            base: [0.0; 3],
            arm_length_m: 2.0,
            swing_deg: 90.0,
            ..Default::default()
        };
        assert!(close(c.arm_tip(), [-2.0, 0.0, 0.0]));
    }

    #[test]
    fn crane_aim_at_hits_target() {
        let mut c = CraneRig {
            base: [0.0, 2.0, 0.0],
            arm_length_m: 3.0,
            boom_deg: 20.0,
            swing_deg: 35.0,
            ..Default::default()
        };
        let target = [1.0, 0.5, -6.0];
        c.aim_at(target);
        let t = c.camera_transform();
        let want = (DVec3::from_array(target) - DVec3::from_array(t.translate)).normalize();
        assert!((forward_of(&t) - want).length() < 1e-9);
    }

    #[test]
    fn dolly_endpoints_and_facing() {
        let mut d = DollyTrack::new(vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]]);
        d.camera_offset = [0.0; 3];
        assert!(close(d.position(0.0), [0.0; 3]));
        assert!(close(d.position(1.0), [10.0, 0.0, 0.0]));
        assert!(close(d.position(0.5), [5.0, 0.0, 0.0]));
        let t = d.at(0.5);
        assert!(close(forward_of(&t).to_array(), [1.0, 0.0, 0.0]));
        assert!((d.length_m(16) - 10.0).abs() < 1e-9);
        d.look_at = Some([5.0, 0.0, -5.0]);
        let t = d.at(0.5);
        assert!(close(forward_of(&t).to_array(), [0.0, 0.0, -1.0]));
        assert!(DollyTrack::new(vec![[0.0; 3]]).validate().is_err());
    }

    #[test]
    fn track_sampling() {
        let mut tr: Track<f64> = Track::new();
        tr.insert(Keyframe::new(2.0, 10.0));
        tr.insert(Keyframe::new(0.0, 0.0));
        tr.insert(Keyframe::with_ease(4.0, 20.0, Ease::Hold));
        tr.insert(Keyframe::new(6.0, 30.0));
        assert_eq!(tr.keys[0].time_s, 0.0);
        assert_eq!(tr.sample(-1.0), Some(0.0));
        assert_eq!(tr.sample(1.0), Some(5.0));
        assert_eq!(tr.sample(2.0), Some(10.0));
        assert_eq!(tr.sample(5.0), Some(20.0)); // hold
        assert_eq!(tr.sample(99.0), Some(30.0));
        assert!(Track::<f64>::new().sample(0.0).is_none());
        assert!((Ease::EaseInOut.apply(0.5) - 0.5).abs() < 1e-12);
        assert!(Ease::EaseInOut.apply(0.25) < 0.25);
    }

    #[test]
    fn animation_blends_transform_and_lens() {
        let mut a = Animation::new();
        a.key_transform(0.0, Transform::at(0.0, 0.0, 0.0), Ease::Linear);
        a.key_transform(2.0, Transform::at(4.0, 0.0, 0.0), Ease::Linear);
        let p0 = CameraParams {
            focal_length_mm: 24.0,
            ..Default::default()
        };
        let p1 = CameraParams {
            focal_length_mm: 85.0,
            ..p0
        };
        a.key_camera(0.0, p0, Ease::Linear);
        a.key_camera(2.0, p1, Ease::Linear);
        let s = a.sample(1.0);
        assert!(close(s.transform.unwrap().translate, [2.0, 0.0, 0.0]));
        assert!((s.camera.unwrap().focal_length_mm - 54.5).abs() < 1e-9);
        assert_eq!(a.duration_s(), 2.0);
        let path = Rig::Animated(a).path(3);
        assert_eq!(path.len(), 3);
        assert!(close(path[2].translate, [4.0, 0.0, 0.0]));
    }

    #[test]
    fn crane_move_path() {
        let from = CraneRig::default();
        let to = CraneRig {
            boom_deg: 45.0,
            ..CraneRig::default()
        };
        let rig = Rig::CraneMove {
            from,
            to,
            ease: Ease::EaseInOut,
        };
        let path = rig.path(5);
        assert_eq!(path.len(), 5);
        assert!(path[4].translate[1] > path[0].translate[1]);
        assert_eq!(Rig::Static(Transform::default()).path(0).len(), 1);
    }
}
