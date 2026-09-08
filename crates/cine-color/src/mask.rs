//! Secondary isolation: geometric masks (power windows) and HSL qualifiers.
//!
//! Mask coordinates are normalized: `(0, 0)` is the top-left corner and
//! `(1, 1)` the bottom-right, independent of the pixel aspect ratio (a circle
//! is specified with equal normalized radii and therefore renders as an
//! ellipse on non-square images unless `radius` accounts for it).

use crate::curve::rgb_to_hsv;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// Boolean combination of two masks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CombineMode {
    /// `max(a, b)`.
    Union,
    /// `a * b`.
    Intersect,
    /// `a * (1 - b)`.
    Subtract,
    /// `a + b - 2ab`.
    Xor,
}

/// A cubic Bézier control point (anchor with incoming / outgoing handles).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BezierPoint {
    pub anchor: [f32; 2],
    pub handle_in: [f32; 2],
    pub handle_out: [f32; 2],
}

/// A geometric mask.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum Mask {
    /// Elliptical window.
    Ellipse {
        center: [f32; 2],
        radius: [f32; 2],
        rotation_deg: f32,
        softness: f32,
        invert: bool,
    },
    /// Rectangular window (`size` is full width/height).
    Rectangle {
        center: [f32; 2],
        size: [f32; 2],
        rotation_deg: f32,
        softness: f32,
        invert: bool,
    },
    /// Closed polygon.
    Polygon {
        points: Vec<[f32; 2]>,
        softness: f32,
        invert: bool,
    },
    /// Closed cubic Bézier path (flattened to a polygon at rasterization).
    Bezier {
        points: Vec<BezierPoint>,
        softness: f32,
        invert: bool,
    },
    /// Linear gradient: 0 at `start`, 1 at `end` (clamped).
    LinearGradient {
        start: [f32; 2],
        end: [f32; 2],
        invert: bool,
    },
    /// Radial gradient: 1 at `center`, falling to 0 at `radius`; `softness`
    /// controls where the falloff starts (0 = hard edge at radius, 1 = falloff from center).
    RadialGradient {
        center: [f32; 2],
        radius: [f32; 2],
        softness: f32,
        invert: bool,
    },
    /// Combination of two masks.
    Combine {
        a: Box<Mask>,
        b: Box<Mask>,
        mode: CombineMode,
    },
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 - e0 <= 1e-9 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn rotate(p: [f32; 2], c: [f32; 2], deg: f32) -> [f32; 2] {
    let (s, co) = deg.to_radians().sin_cos();
    let dx = p[0] - c[0];
    let dy = p[1] - c[1];
    [co * dx + s * dy, -s * dx + co * dy]
}

/// Signed distance from `p` to the polygon boundary (negative inside).
fn polygon_sdf(p: [f32; 2], pts: &[[f32; 2]]) -> f32 {
    let n = pts.len();
    if n < 3 {
        return f32::INFINITY;
    }
    let mut d2 = f32::INFINITY;
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let a = pts[i];
        let b = pts[j];
        let e = [b[0] - a[0], b[1] - a[1]];
        let w = [p[0] - a[0], p[1] - a[1]];
        let len2 = e[0] * e[0] + e[1] * e[1];
        let t = if len2 < 1e-12 {
            0.0
        } else {
            ((w[0] * e[0] + w[1] * e[1]) / len2).clamp(0.0, 1.0)
        };
        let q = [w[0] - e[0] * t, w[1] - e[1] * t];
        d2 = d2.min(q[0] * q[0] + q[1] * q[1]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                inside = !inside;
            }
        }
        j = i;
    }
    let d = d2.sqrt();
    if inside {
        -d
    } else {
        d
    }
}

fn flatten_bezier(points: &[BezierPoint], segments: usize) -> Vec<[f32; 2]> {
    let n = points.len();
    let mut out = Vec::with_capacity(n * segments);
    for i in 0..n {
        let p0 = points[i];
        let p3 = points[(i + 1) % n];
        let a = p0.anchor;
        let b = p0.handle_out;
        let c = p3.handle_in;
        let d = p3.anchor;
        for s in 0..segments {
            let t = s as f32 / segments as f32;
            let u = 1.0 - t;
            let x = u * u * u * a[0]
                + 3.0 * u * u * t * b[0]
                + 3.0 * u * t * t * c[0]
                + t * t * t * d[0];
            let y = u * u * u * a[1]
                + 3.0 * u * u * t * b[1]
                + 3.0 * u * t * t * c[1]
                + t * t * t * d[1];
            out.push([x, y]);
        }
    }
    out
}

impl Mask {
    /// Circle helper.
    pub fn circle(center: [f32; 2], radius: f32, softness: f32) -> Mask {
        Mask::Ellipse {
            center,
            radius: [radius, radius],
            rotation_deg: 0.0,
            softness,
            invert: false,
        }
    }

    /// Combine two masks.
    pub fn combine(a: Mask, b: Mask, mode: CombineMode) -> Mask {
        Mask::Combine {
            a: Box::new(a),
            b: Box::new(b),
            mode,
        }
    }

    /// Inverted copy (for gradients / shapes; a combine is wrapped in a subtract from a full mask).
    pub fn inverted(self) -> Mask {
        match self {
            Mask::Ellipse {
                center,
                radius,
                rotation_deg,
                softness,
                invert,
            } => Mask::Ellipse {
                center,
                radius,
                rotation_deg,
                softness,
                invert: !invert,
            },
            Mask::Rectangle {
                center,
                size,
                rotation_deg,
                softness,
                invert,
            } => Mask::Rectangle {
                center,
                size,
                rotation_deg,
                softness,
                invert: !invert,
            },
            Mask::Polygon {
                points,
                softness,
                invert,
            } => Mask::Polygon {
                points,
                softness,
                invert: !invert,
            },
            Mask::Bezier {
                points,
                softness,
                invert,
            } => Mask::Bezier {
                points,
                softness,
                invert: !invert,
            },
            Mask::LinearGradient { start, end, invert } => Mask::LinearGradient {
                start,
                end,
                invert: !invert,
            },
            Mask::RadialGradient {
                center,
                radius,
                softness,
                invert,
            } => Mask::RadialGradient {
                center,
                radius,
                softness,
                invert: !invert,
            },
            c @ Mask::Combine { .. } => Mask::combine(
                Mask::Rectangle {
                    center: [0.5, 0.5],
                    size: [4.0, 4.0],
                    rotation_deg: 0.0,
                    softness: 0.0,
                    invert: false,
                },
                c,
                CombineMode::Subtract,
            ),
        }
    }

    /// Mask value in [0, 1] at a normalized coordinate.
    pub fn eval(&self, p: [f32; 2]) -> f32 {
        match self {
            Mask::Ellipse {
                center,
                radius,
                rotation_deg,
                softness,
                invert,
            } => {
                let q = rotate(p, *center, *rotation_deg);
                let rx = radius[0].max(1e-6);
                let ry = radius[1].max(1e-6);
                let d = ((q[0] / rx).powi(2) + (q[1] / ry).powi(2)).sqrt();
                let v = 1.0 - smoothstep(1.0, 1.0 + softness.max(0.0), d);
                if *invert {
                    1.0 - v
                } else {
                    v
                }
            }
            Mask::Rectangle {
                center,
                size,
                rotation_deg,
                softness,
                invert,
            } => {
                let q = rotate(p, *center, *rotation_deg);
                let hx = size[0] * 0.5;
                let hy = size[1] * 0.5;
                let dx = q[0].abs() - hx;
                let dy = q[1].abs() - hy;
                let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
                let sd = outside + dx.max(dy).min(0.0);
                let v = 1.0 - smoothstep(0.0, softness.max(0.0), sd);
                if *invert {
                    1.0 - v
                } else {
                    v
                }
            }
            Mask::Polygon {
                points,
                softness,
                invert,
            } => {
                let sd = polygon_sdf(p, points);
                let v = 1.0 - smoothstep(0.0, softness.max(0.0), sd);
                if *invert {
                    1.0 - v
                } else {
                    v
                }
            }
            Mask::Bezier {
                points,
                softness,
                invert,
            } => {
                let poly = flatten_bezier(points, 16);
                let sd = polygon_sdf(p, &poly);
                let v = 1.0 - smoothstep(0.0, softness.max(0.0), sd);
                if *invert {
                    1.0 - v
                } else {
                    v
                }
            }
            Mask::LinearGradient { start, end, invert } => {
                let e = [end[0] - start[0], end[1] - start[1]];
                let len2 = e[0] * e[0] + e[1] * e[1];
                let t = if len2 < 1e-12 {
                    1.0
                } else {
                    (((p[0] - start[0]) * e[0] + (p[1] - start[1]) * e[1]) / len2).clamp(0.0, 1.0)
                };
                if *invert {
                    1.0 - t
                } else {
                    t
                }
            }
            Mask::RadialGradient {
                center,
                radius,
                softness,
                invert,
            } => {
                let rx = radius[0].max(1e-6);
                let ry = radius[1].max(1e-6);
                let d =
                    (((p[0] - center[0]) / rx).powi(2) + ((p[1] - center[1]) / ry).powi(2)).sqrt();
                let inner = (1.0 - softness.clamp(0.0, 1.0)).max(0.0);
                let v = 1.0 - smoothstep(inner, 1.0, d);
                if *invert {
                    1.0 - v
                } else {
                    v
                }
            }
            Mask::Combine { a, b, mode } => {
                let va = a.eval(p);
                let vb = b.eval(p);
                match mode {
                    CombineMode::Union => va.max(vb),
                    CombineMode::Intersect => va * vb,
                    CombineMode::Subtract => va * (1.0 - vb),
                    CombineMode::Xor => va + vb - 2.0 * va * vb,
                }
            }
        }
    }

    /// Rasterize to a `w × h` coverage buffer (row-major, pixel centers).
    pub fn rasterize(&self, w: usize, h: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; w * h];
        if w == 0 || h == 0 {
            return out;
        }
        // Pre-flatten Béziers once.
        let flat = match self {
            Mask::Bezier {
                points,
                softness,
                invert,
            } => Some(Mask::Polygon {
                points: flatten_bezier(points, 16),
                softness: *softness,
                invert: *invert,
            }),
            _ => None,
        };
        let m = flat.as_ref().unwrap_or(self);
        out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let py = (y as f32 + 0.5) / h as f32;
            for (x, v) in row.iter_mut().enumerate() {
                let px = (x as f32 + 0.5) / w as f32;
                *v = m.eval([px, py]);
            }
        });
        out
    }
}

/// Selects pixels by hue / saturation / luma ranges with soft edges.
///
/// Hue is in degrees (0 = red, 120 = green, 240 = blue); saturation is HSV
/// saturation in [0, 1]; luma is Rec.709 luma. Each `*_soft` is the width of
/// the smooth falloff outside the range (same units as the range).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HslQualifier {
    pub hue_center_deg: f32,
    pub hue_width_deg: f32,
    pub hue_soft_deg: f32,
    pub sat_range: [f32; 2],
    pub sat_soft: f32,
    pub lum_range: [f32; 2],
    pub lum_soft: f32,
    pub invert: bool,
}

impl Default for HslQualifier {
    fn default() -> Self {
        HslQualifier {
            hue_center_deg: 0.0,
            hue_width_deg: 360.0,
            hue_soft_deg: 0.0,
            sat_range: [0.0, 1.0],
            sat_soft: 0.0,
            lum_range: [0.0, f32::MAX],
            lum_soft: 0.0,
            invert: false,
        }
    }
}

fn range_weight(x: f32, lo: f32, hi: f32, soft: f32) -> f32 {
    if soft <= 1e-9 {
        return if x >= lo && x <= hi { 1.0 } else { 0.0 };
    }
    let a = smoothstep(lo - soft, lo, x);
    let b = 1.0 - smoothstep(hi, hi + soft, x);
    a * b
}

impl HslQualifier {
    /// Qualifier selecting a hue band (full saturation / luma range).
    pub fn hue(center_deg: f32, width_deg: f32, soft_deg: f32) -> HslQualifier {
        HslQualifier {
            hue_center_deg: center_deg,
            hue_width_deg: width_deg,
            hue_soft_deg: soft_deg,
            ..Default::default()
        }
    }

    /// Restrict saturation (builder).
    pub fn with_sat(mut self, lo: f32, hi: f32, soft: f32) -> Self {
        self.sat_range = [lo, hi];
        self.sat_soft = soft;
        self
    }

    /// Restrict luma (builder).
    pub fn with_lum(mut self, lo: f32, hi: f32, soft: f32) -> Self {
        self.lum_range = [lo, hi];
        self.lum_soft = soft;
        self
    }

    /// Selection weight of one pixel in [0, 1].
    pub fn weight(&self, rgb: [f32; 3]) -> f32 {
        let (h, s, _v) = rgb_to_hsv(rgb);
        let luma = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
        // hue: distance to the center on the circle
        let hd = h * 360.0 - self.hue_center_deg;
        let hd = ((hd + 180.0).rem_euclid(360.0) - 180.0).abs();
        let half = self.hue_width_deg * 0.5;
        let wh = if self.hue_width_deg >= 360.0 {
            1.0
        } else if self.hue_soft_deg <= 1e-6 {
            if hd <= half {
                1.0
            } else {
                0.0
            }
        } else {
            1.0 - smoothstep(half, half + self.hue_soft_deg, hd)
        };
        let ws = range_weight(s, self.sat_range[0], self.sat_range[1], self.sat_soft);
        let wl = range_weight(luma, self.lum_range[0], self.lum_range[1], self.lum_soft);
        let w = wh * ws * wl;
        if self.invert {
            1.0 - w
        } else {
            w
        }
    }

    /// Mask for an interleaved image.
    pub fn mask(&self, data: &[f32], channels: usize) -> Vec<f32> {
        if channels < 3 {
            return vec![0.0; data.len() / channels.max(1)];
        }
        data.par_chunks(channels)
            .map(|px| self.weight([px[0], px[1], px[2]]))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ellipse_area() {
        let m = Mask::circle([0.5, 0.5], 0.25, 0.0);
        let cov = m.rasterize(200, 200);
        let sum: f32 = cov.iter().sum();
        let want = std::f32::consts::PI * 50.0 * 50.0;
        assert!((sum - want).abs() / want < 0.01, "{sum} vs {want}");
    }

    #[test]
    fn rectangle_area_and_invert() {
        let m = Mask::Rectangle {
            center: [0.5, 0.5],
            size: [0.5, 0.25],
            rotation_deg: 0.0,
            softness: 0.0,
            invert: false,
        };
        let cov = m.rasterize(100, 100);
        let sum: f32 = cov.iter().sum();
        assert!((sum - 50.0 * 25.0).abs() < 60.0, "{sum}");
        let inv = m.inverted().rasterize(100, 100);
        let s2: f32 = inv.iter().sum();
        assert!((s2 - (10_000.0 - sum)).abs() < 1e-3);
    }

    #[test]
    fn polygon_triangle_area() {
        let m = Mask::Polygon {
            points: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            softness: 0.0,
            invert: false,
        };
        let cov = m.rasterize(100, 100);
        let sum: f32 = cov.iter().sum();
        assert!((sum - 5000.0).abs() < 100.0, "{sum}");
    }

    #[test]
    fn soft_edge_is_smooth() {
        let m = Mask::circle([0.5, 0.5], 0.25, 0.5);
        assert!((m.eval([0.5, 0.5]) - 1.0).abs() < 1e-6);
        let mid = m.eval([0.5 + 0.25 * 1.25, 0.5]);
        assert!(mid > 0.1 && mid < 0.9, "{mid}");
        assert!(m.eval([0.5 + 0.25 * 1.6, 0.5]) < 1e-6);
    }

    #[test]
    fn gradients_and_combine() {
        let g = Mask::LinearGradient {
            start: [0.0, 0.0],
            end: [1.0, 0.0],
            invert: false,
        };
        assert!((g.eval([0.25, 0.7]) - 0.25).abs() < 1e-6);
        let r = Mask::RadialGradient {
            center: [0.5, 0.5],
            radius: [0.5, 0.5],
            softness: 1.0,
            invert: false,
        };
        assert!((r.eval([0.5, 0.5]) - 1.0).abs() < 1e-6);
        assert!(r.eval([1.0, 0.5]) < 1e-6);
        let a = Mask::circle([0.3, 0.5], 0.2, 0.0);
        let b = Mask::circle([0.7, 0.5], 0.2, 0.0);
        let u = Mask::combine(a.clone(), b.clone(), CombineMode::Union);
        assert_eq!(u.eval([0.3, 0.5]), 1.0);
        assert_eq!(u.eval([0.7, 0.5]), 1.0);
        let i = Mask::combine(a, b, CombineMode::Intersect);
        assert_eq!(i.eval([0.3, 0.5]), 0.0);
    }

    #[test]
    fn bezier_circle_area() {
        // 4-point Bézier approximation of a circle radius 0.25.
        let k = 0.5523 * 0.25;
        let c = [0.5f32, 0.5f32];
        let pts = vec![
            BezierPoint {
                anchor: [c[0] + 0.25, c[1]],
                handle_in: [c[0] + 0.25, c[1] - k],
                handle_out: [c[0] + 0.25, c[1] + k],
            },
            BezierPoint {
                anchor: [c[0], c[1] + 0.25],
                handle_in: [c[0] + k, c[1] + 0.25],
                handle_out: [c[0] - k, c[1] + 0.25],
            },
            BezierPoint {
                anchor: [c[0] - 0.25, c[1]],
                handle_in: [c[0] - 0.25, c[1] + k],
                handle_out: [c[0] - 0.25, c[1] - k],
            },
            BezierPoint {
                anchor: [c[0], c[1] - 0.25],
                handle_in: [c[0] - k, c[1] - 0.25],
                handle_out: [c[0] + k, c[1] - 0.25],
            },
        ];
        let m = Mask::Bezier {
            points: pts,
            softness: 0.0,
            invert: false,
        };
        let sum: f32 = m.rasterize(200, 200).iter().sum();
        let want = std::f32::consts::PI * 50.0 * 50.0;
        assert!((sum - want).abs() / want < 0.02, "{sum} vs {want}");
    }

    #[test]
    fn qualifier_selects_green_only() {
        let q = HslQualifier::hue(120.0, 60.0, 10.0).with_sat(0.3, 1.0, 0.05);
        assert!(q.weight([0.1, 0.8, 0.1]) > 0.99);
        assert!(q.weight([0.8, 0.1, 0.1]) < 1e-6);
        assert!(q.weight([0.1, 0.1, 0.8]) < 1e-6);
        assert!(q.weight([0.5, 0.5, 0.5]) < 1e-6);
        let inv = HslQualifier { invert: true, ..q };
        assert!(inv.weight([0.8, 0.1, 0.1]) > 0.99);
    }
}
