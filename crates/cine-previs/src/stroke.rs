//! Hybrid grease-pencil strokes: drawn in the 2D camera frame, projected
//! onto a plane in front of the camera, rendered as 3D ribbons.
//!
//! Frame coordinates follow [`crate::optics`]: `0..1`, origin top-left,
//! `y` down. A stroke projected at `depth_m` lands on the plane
//! perpendicular to the camera axis at that distance, so it stays glued to
//! the camera framing while living in world space.

use glam::DVec3;
use serde::{Deserialize, Serialize};
use ucg_schema::{CameraParams, Transform};

use crate::optics::{forward_of, unproject_frame_point, world_to_frame_depth};

/// One sample of a tablet stroke in normalized frame coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct StrokePoint {
    /// Horizontal frame coordinate (`0` = left, `1` = right).
    pub x: f32,
    /// Vertical frame coordinate (`0` = top, `1` = bottom).
    pub y: f32,
    /// Pen pressure `0..1`.
    pub pressure: f32,
    /// Pen tilt (x, y) in `-1..1`.
    pub tilt: [f32; 2],
    /// Milliseconds since the stroke started.
    pub time_ms: u32,
}

impl StrokePoint {
    /// A point with full pressure and no tilt.
    pub fn new(x: f32, y: f32) -> Self {
        StrokePoint {
            x,
            y,
            pressure: 1.0,
            tilt: [0.0, 0.0],
            time_ms: 0,
        }
    }
    /// A point with explicit pressure.
    pub fn with_pressure(x: f32, y: f32, pressure: f32) -> Self {
        StrokePoint {
            x,
            y,
            pressure,
            tilt: [0.0, 0.0],
            time_ms: 0,
        }
    }
}

/// A 2D stroke on a storyboard layer / camera frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    /// Samples in drawing order.
    pub points: Vec<StrokePoint>,
    /// Linear RGBA colour.
    pub color: [f32; 4],
    /// Nominal width in pixels at full pressure.
    pub width_px: f32,
    /// Layer index on the canvas.
    pub layer: u32,
    /// Storyboard / timeline frame the stroke belongs to.
    pub frame: i64,
}

impl Default for Stroke {
    fn default() -> Self {
        Stroke {
            points: Vec::new(),
            color: [0.0, 0.0, 0.0, 1.0],
            width_px: 4.0,
            layer: 0,
            frame: 0,
        }
    }
}

impl Stroke {
    /// A black 4 px stroke through the given frame points.
    pub fn from_points(points: Vec<StrokePoint>) -> Self {
        Stroke {
            points,
            ..Default::default()
        }
    }
    /// Number of samples.
    pub fn len(&self) -> usize {
        self.points.len()
    }
    /// Whether the stroke has no samples.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
    /// Polyline length in frame units.
    pub fn length(&self) -> f64 {
        self.points
            .windows(2)
            .map(|w| {
                let dx = (w[1].x - w[0].x) as f64;
                let dy = (w[1].y - w[0].y) as f64;
                (dx * dx + dy * dy).sqrt()
            })
            .sum()
    }
    /// Axis-aligned bounds `(min, max)` in frame coordinates.
    pub fn bounds(&self) -> Option<([f32; 2], [f32; 2])> {
        let first = self.points.first()?;
        let mut min = [first.x, first.y];
        let mut max = min;
        for p in &self.points {
            min = [min[0].min(p.x), min[1].min(p.y)];
            max = [max[0].max(p.x), max[1].max(p.y)];
        }
        Some((min, max))
    }
    /// Douglas–Peucker simplification with `tolerance` in frame units.
    pub fn simplify(&self, tolerance: f64) -> Stroke {
        let pts: Vec<[f64; 2]> = self
            .points
            .iter()
            .map(|p| [p.x as f64, p.y as f64])
            .collect();
        let keep = douglas_peucker(&pts, tolerance, point_segment_distance_2d);
        Stroke {
            points: keep.into_iter().map(|i| self.points[i]).collect(),
            ..self.clone()
        }
    }
    /// Laplacian smoothing (endpoints fixed), `iterations` passes.
    pub fn smooth(&self, iterations: usize) -> Stroke {
        let mut pts = self.points.clone();
        for _ in 0..iterations {
            if pts.len() < 3 {
                break;
            }
            let prev = pts.clone();
            for i in 1..pts.len() - 1 {
                pts[i].x = 0.25 * prev[i - 1].x + 0.5 * prev[i].x + 0.25 * prev[i + 1].x;
                pts[i].y = 0.25 * prev[i - 1].y + 0.5 * prev[i].y + 0.25 * prev[i + 1].y;
                pts[i].pressure = 0.25 * prev[i - 1].pressure
                    + 0.5 * prev[i].pressure
                    + 0.25 * prev[i + 1].pressure;
            }
        }
        Stroke {
            points: pts,
            ..self.clone()
        }
    }
}

/// GPU-ready ribbon geometry: a triangle strip with two vertices per
/// stroke sample.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Ribbon {
    /// Vertex positions (world space, metres).
    pub positions: Vec<[f32; 3]>,
    /// Texture coordinates: `u` along the stroke `0..1`, `v` across `0/1`.
    pub uvs: Vec<[f32; 2]>,
    /// Triangle-strip indices (`2 × samples`).
    pub indices: Vec<u32>,
}

impl Ribbon {
    /// Number of vertices.
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }
    /// Number of triangles in the strip.
    pub fn triangle_count(&self) -> usize {
        self.indices.len().saturating_sub(2)
    }
    /// The same geometry as a triangle list (3 indices per triangle).
    pub fn triangle_list_indices(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.triangle_count() * 3);
        for i in 0..self.triangle_count() {
            let (a, b, c) = (self.indices[i], self.indices[i + 1], self.indices[i + 2]);
            if i % 2 == 0 {
                out.extend_from_slice(&[a, b, c]);
            } else {
                out.extend_from_slice(&[b, a, c]);
            }
        }
        out
    }
}

/// A stroke lifted into world space.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke3D {
    /// World positions (metres), one per sample.
    pub points: Vec<[f64; 3]>,
    /// Pen pressure per sample.
    pub pressures: Vec<f32>,
    /// Pen tilt per sample.
    pub tilts: Vec<[f32; 2]>,
    /// Timestamp per sample.
    pub times_ms: Vec<u32>,
    /// Normal of the plane the stroke was projected on (camera forward).
    pub normal: [f64; 3],
    /// Distance from the camera to the projection plane.
    pub depth_m: f64,
    /// Linear RGBA colour.
    pub color: [f32; 4],
    /// Nominal width in pixels (kept for re-projection).
    pub width_px: f32,
    /// Source layer.
    pub layer: u32,
    /// Source frame.
    pub frame: i64,
}

impl Stroke3D {
    /// Number of samples.
    pub fn len(&self) -> usize {
        self.points.len()
    }
    /// Whether the stroke has no samples.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
    /// Polyline length in metres.
    pub fn length_m(&self) -> f64 {
        self.points
            .windows(2)
            .map(|w| (DVec3::from_array(w[1]) - DVec3::from_array(w[0])).length())
            .sum()
    }

    fn select(&self, keep: &[usize]) -> Stroke3D {
        Stroke3D {
            points: keep.iter().map(|&i| self.points[i]).collect(),
            pressures: keep.iter().map(|&i| self.pressures[i]).collect(),
            tilts: keep.iter().map(|&i| self.tilts[i]).collect(),
            times_ms: keep.iter().map(|&i| self.times_ms[i]).collect(),
            ..self.clone()
        }
    }

    /// Douglas–Peucker simplification with `tolerance` in metres.
    pub fn simplify(&self, tolerance: f64) -> Stroke3D {
        let keep = douglas_peucker(&self.points, tolerance, point_segment_distance_3d);
        self.select(&keep)
    }

    /// Laplacian smoothing (endpoints fixed), `iterations` passes.
    pub fn smooth(&self, iterations: usize) -> Stroke3D {
        let mut pts = self.points.clone();
        for _ in 0..iterations {
            if pts.len() < 3 {
                break;
            }
            let prev = pts.clone();
            for i in 1..pts.len() - 1 {
                for k in 0..3 {
                    pts[i][k] = 0.25 * prev[i - 1][k] + 0.5 * prev[i][k] + 0.25 * prev[i + 1][k];
                }
            }
        }
        Stroke3D {
            points: pts,
            ..self.clone()
        }
    }

    /// Build a flat ribbon of `width_m` (scaled by pressure) lying in the
    /// projection plane. Vertex count is `2 × len()`; an empty or
    /// single-point stroke yields an empty ribbon.
    pub fn to_ribbon(&self, width_m: f64) -> Ribbon {
        let n = self.points.len();
        if n < 2 {
            return Ribbon::default();
        }
        let normal = DVec3::from_array(self.normal).normalize_or_zero();
        let normal = if normal.length_squared() < 0.5 {
            DVec3::Z
        } else {
            normal
        };
        let total = self.length_m().max(1e-12);
        let mut ribbon = Ribbon {
            positions: Vec::with_capacity(2 * n),
            uvs: Vec::with_capacity(2 * n),
            indices: Vec::with_capacity(2 * n),
        };
        let mut walked = 0.0;
        for i in 0..n {
            let p = DVec3::from_array(self.points[i]);
            let prev = DVec3::from_array(self.points[i.saturating_sub(1)]);
            let next = DVec3::from_array(self.points[(i + 1).min(n - 1)]);
            if i > 0 {
                walked += (p - prev).length();
            }
            let tangent = (next - prev).normalize_or_zero();
            let mut side = tangent.cross(normal).normalize_or_zero();
            if side.length_squared() < 0.5 {
                side = normal.any_orthonormal_vector();
            }
            let half =
                0.5 * width_m * self.pressures.get(i).copied().unwrap_or(1.0).max(0.05) as f64;
            let u = (walked / total) as f32;
            ribbon
                .positions
                .push((p - side * half).as_vec3().to_array());
            ribbon
                .positions
                .push((p + side * half).as_vec3().to_array());
            ribbon.uvs.push([u, 0.0]);
            ribbon.uvs.push([u, 1.0]);
            ribbon.indices.push((2 * i) as u32);
            ribbon.indices.push((2 * i + 1) as u32);
        }
        ribbon
    }
}

/// Project a frame-space stroke onto the plane `depth_m` in front of the
/// camera. Points outside `0..1` land outside the frame but are still
/// projected.
pub fn project_stroke_to_plane(
    stroke: &Stroke,
    camera: &Transform,
    params: &CameraParams,
    depth_m: f64,
) -> Stroke3D {
    let points = stroke
        .points
        .iter()
        .map(|p| unproject_frame_point(params, camera, [p.x as f64, p.y as f64], depth_m))
        .collect();
    Stroke3D {
        points,
        pressures: stroke.points.iter().map(|p| p.pressure).collect(),
        tilts: stroke.points.iter().map(|p| p.tilt).collect(),
        times_ms: stroke.points.iter().map(|p| p.time_ms).collect(),
        normal: forward_of(camera).to_array(),
        depth_m,
        color: stroke.color,
        width_px: stroke.width_px,
        layer: stroke.layer,
        frame: stroke.frame,
    }
}

/// Inverse of [`project_stroke_to_plane`]: bring a world stroke back into
/// the frame of (possibly another) camera. Samples behind the camera are
/// dropped.
pub fn reproject_to_frame(stroke: &Stroke3D, camera: &Transform, params: &CameraParams) -> Stroke {
    let mut points = Vec::with_capacity(stroke.points.len());
    for (i, w) in stroke.points.iter().enumerate() {
        if let Some((f, _depth)) = world_to_frame_depth(params, camera, *w) {
            points.push(StrokePoint {
                x: f[0] as f32,
                y: f[1] as f32,
                pressure: stroke.pressures.get(i).copied().unwrap_or(1.0),
                tilt: stroke.tilts.get(i).copied().unwrap_or([0.0, 0.0]),
                time_ms: stroke.times_ms.get(i).copied().unwrap_or(0),
            });
        }
    }
    Stroke {
        points,
        color: stroke.color,
        width_px: stroke.width_px,
        layer: stroke.layer,
        frame: stroke.frame,
    }
}

// ---------------------------------------------------------------------------
// Geometry helpers
// ---------------------------------------------------------------------------

/// Distance from `p` to segment `ab` in 2D.
pub fn point_segment_distance_2d(p: &[f64; 2], a: &[f64; 2], b: &[f64; 2]) -> f64 {
    let (ax, ay, bx, by) = (a[0], a[1], b[0], b[1]);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= 0.0 {
        0.0
    } else {
        (((p[0] - ax) * dx + (p[1] - ay) * dy) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt()
}

/// Distance from `p` to segment `ab` in 3D.
pub fn point_segment_distance_3d(p: &[f64; 3], a: &[f64; 3], b: &[f64; 3]) -> f64 {
    let (p, a, b) = (
        DVec3::from_array(*p),
        DVec3::from_array(*a),
        DVec3::from_array(*b),
    );
    let d = b - a;
    let len2 = d.length_squared();
    let t = if len2 <= 0.0 {
        0.0
    } else {
        ((p - a).dot(d) / len2).clamp(0.0, 1.0)
    };
    (p - (a + d * t)).length()
}

/// Generic Douglas–Peucker: returns the indices to keep (always includes
/// the first and last point).
pub fn douglas_peucker<P>(points: &[P], tolerance: f64, dist: fn(&P, &P, &P) -> f64) -> Vec<usize> {
    let n = points.len();
    if n <= 2 {
        return (0..n).collect();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((s, e)) = stack.pop() {
        if e <= s + 1 {
            continue;
        }
        let mut best = 0.0;
        let mut idx = s;
        for (i, p) in points.iter().enumerate().take(e).skip(s + 1) {
            let d = dist(p, &points[s], &points[e]);
            if d > best {
                best = d;
                idx = i;
            }
        }
        if best > tolerance {
            keep[idx] = true;
            stack.push((s, idx));
            stack.push((idx, e));
        }
    }
    keep.iter()
        .enumerate()
        .filter(|(_, k)| **k)
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dp_keeps_corners_drops_collinear() {
        let pts: Vec<[f64; 2]> = (0..11).map(|i| [i as f64 * 0.1, 0.0]).collect();
        assert_eq!(
            douglas_peucker(&pts, 1e-6, point_segment_distance_2d),
            vec![0, 10]
        );
        let pts = vec![[0.0, 0.0], [0.5, 0.5], [1.0, 0.0]];
        assert_eq!(
            douglas_peucker(&pts, 0.1, point_segment_distance_2d),
            vec![0, 1, 2]
        );
        assert!(
            (point_segment_distance_3d(&[0.0, 1.0, 0.0], &[-1.0, 0.0, 0.0], &[1.0, 0.0, 0.0])
                - 1.0)
                .abs()
                < 1e-12
        );
    }

    #[test]
    fn smoothing_pulls_in_the_middle() {
        let s = Stroke::from_points(vec![
            StrokePoint::new(0.0, 0.0),
            StrokePoint::new(0.5, 1.0),
            StrokePoint::new(1.0, 0.0),
        ]);
        let sm = s.smooth(1);
        assert!(sm.points[1].y < 1.0);
        assert_eq!(sm.points[0], s.points[0]);
        assert_eq!(sm.points[2], s.points[2]);
    }

    #[test]
    fn strip_to_list() {
        let r = Ribbon {
            positions: vec![[0.0; 3]; 4],
            uvs: vec![[0.0; 2]; 4],
            indices: vec![0, 1, 2, 3],
        };
        assert_eq!(r.triangle_count(), 2);
        assert_eq!(r.triangle_list_indices(), vec![0, 1, 2, 2, 1, 3]);
    }
}
