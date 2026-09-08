//! Roto: rasterize polygon / Bezier shapes to a feathered mask.
//!
//! Each shape is flattened to a polygon; per pixel the non-zero winding rule
//! decides inside/outside and the signed distance to the outline drives the
//! feather ramp (`alpha = clamp(0.5 - sd / feather)`), so a feathered edge
//! is centered on the outline and area is preserved on average.

use crate::graph_model::Shape;
use crate::image::{ChannelSet, Image};
use rayon::prelude::*;

/// Subdivisions per Bezier segment when flattening.
const BEZIER_STEPS: usize = 24;

/// Flatten a shape to a closed polygon.
pub fn flatten(shape: &Shape) -> Vec<[f32; 2]> {
    if !shape.bezier || shape.points.len() < 3 {
        return shape.points.clone();
    }
    let pts = &shape.points;
    let n = pts.len() / 3;
    let mut out = Vec::with_capacity(n * BEZIER_STEPS);
    for s in 0..n {
        let p0 = pts[s * 3];
        let c1 = pts[s * 3 + 1];
        let c2 = pts[s * 3 + 2];
        let p1 = pts[((s + 1) * 3) % pts.len()];
        for i in 0..BEZIER_STEPS {
            let t = i as f32 / BEZIER_STEPS as f32;
            let u = 1.0 - t;
            let x = u * u * u * p0[0]
                + 3.0 * u * u * t * c1[0]
                + 3.0 * u * t * t * c2[0]
                + t * t * t * p1[0];
            let y = u * u * u * p0[1]
                + 3.0 * u * u * t * c1[1]
                + 3.0 * u * t * t * c2[1]
                + t * t * t * p1[1];
            out.push([x, y]);
        }
    }
    out
}

/// Signed area of a polygon (positive for counter-clockwise in y-up, i.e.
/// clockwise on screen); `abs()` is the enclosed area.
pub fn polygon_area(poly: &[[f32; 2]]) -> f32 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..n {
        let p = poly[i];
        let q = poly[(i + 1) % n];
        a += p[0] * q[1] - q[0] * p[1];
    }
    a * 0.5
}

/// Non-zero winding inside test and distance to the outline.
fn signed_distance(poly: &[[f32; 2]], px: f32, py: f32) -> f32 {
    let n = poly.len();
    let mut winding = 0i32;
    let mut min_d2 = f32::INFINITY;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        // distance to segment
        let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
        let len2 = ex * ex + ey * ey;
        let t = if len2 > 0.0 {
            (((px - a[0]) * ex + (py - a[1]) * ey) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (cx, cy) = (a[0] + ex * t - px, a[1] + ey * t - py);
        min_d2 = min_d2.min(cx * cx + cy * cy);
        // winding
        if a[1] <= py {
            if b[1] > py && (ex * (py - a[1]) - (px - a[0]) * ey) > 0.0 {
                winding += 1;
            }
        } else if b[1] <= py && (ex * (py - a[1]) - (px - a[0]) * ey) < 0.0 {
            winding -= 1;
        }
    }
    let d = min_d2.sqrt();
    if winding != 0 {
        -d
    } else {
        d
    }
}

/// Coverage of one shape at pixel center `(px, py)`.
pub fn shape_coverage(poly: &[[f32; 2]], feather: f32, px: f32, py: f32) -> f32 {
    if poly.len() < 3 {
        return 0.0;
    }
    let sd = signed_distance(poly, px, py);
    if feather <= 0.0 {
        if sd < 0.0 {
            1.0
        } else {
            0.0
        }
    } else {
        (0.5 - sd / feather).clamp(0.0, 1.0)
    }
}

/// Rasterize shapes (union, each scaled by its opacity) into an alpha mask.
pub fn rasterize(shapes: &[Shape], width: usize, height: usize, invert: bool) -> Image {
    let polys: Vec<(Vec<[f32; 2]>, f32, f32)> = shapes
        .iter()
        .map(|s| (flatten(s), s.feather, s.opacity))
        .filter(|(p, _, _)| p.len() >= 3)
        .collect();
    let mut out = Image::new(width, height, ChannelSet::Alpha);
    if out.is_empty() {
        return out;
    }
    out.data
        .par_chunks_exact_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            let py = y as f32 + 0.5;
            for (x, o) in row.iter_mut().enumerate() {
                let px = x as f32 + 0.5;
                let mut v = 0.0f32;
                for (poly, feather, opacity) in &polys {
                    v = v.max(shape_coverage(poly, *feather, px, py) * opacity);
                }
                *o = if invert { 1.0 - v } else { v };
            }
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangle_area_exact() {
        let m = rasterize(&[Shape::rect(2.0, 3.0, 10.0, 5.0)], 32, 32, false);
        assert!((m.channel_sum(0) - 50.0).abs() < 1e-4);
        assert_eq!(m.get(2, 3, 0), 1.0);
        assert_eq!(m.get(1, 3, 0), 0.0);
        let inv = rasterize(&[Shape::rect(2.0, 3.0, 10.0, 5.0)], 32, 32, true);
        assert!((inv.channel_sum(0) - (1024.0 - 50.0)).abs() < 1e-3);
    }

    #[test]
    fn feathered_polygon_area_within_two_percent() {
        // triangle with area 0.5 * 40 * 30 = 600
        let tri = Shape::polygon(vec![[10.0, 10.0], [50.0, 10.0], [10.0, 40.0]]).with_feather(3.0);
        let m = rasterize(&[tri], 64, 64, false);
        let area = m.channel_sum(0);
        assert!((area - 600.0).abs() / 600.0 < 0.02, "area {area}");
    }

    #[test]
    fn bezier_circle_area() {
        // 4-segment cubic approximation of a circle radius 20 at (32,32).
        let k = 0.5523 * 20.0;
        let (cx, cy, r) = (32.0, 32.0, 20.0);
        let pts = vec![
            [cx + r, cy],
            [cx + r, cy + k],
            [cx + k, cy + r],
            [cx, cy + r],
            [cx - k, cy + r],
            [cx - r, cy + k],
            [cx - r, cy],
            [cx - r, cy - k],
            [cx - k, cy - r],
            [cx, cy - r],
            [cx + k, cy - r],
            [cx + r, cy - k],
        ];
        let m = rasterize(&[Shape::bezier(pts).with_feather(1.0)], 64, 64, false);
        let area = m.channel_sum(0);
        let expected = std::f64::consts::PI * 400.0;
        assert!(
            (area - expected).abs() / expected < 0.02,
            "area {area} vs {expected}"
        );
        assert!(polygon_area(&flatten(&Shape::rect(0.0, 0.0, 2.0, 3.0))).abs() - 6.0 < 1e-6);
    }
}
