//! Monotone cubic Hermite curves (Fritsch–Carlson) and the grading curve set.

use crate::lut::Lut1D;
use serde::{Deserialize, Serialize};

/// A curve through control points, interpolated with a monotone cubic
/// (Fritsch–Carlson tangents), linearly extrapolated outside the first/last point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    /// `[x, y]` control points; sorted by x on construction.
    pub points: Vec<[f32; 2]>,
}

impl Default for Curve {
    fn default() -> Self {
        Curve::identity()
    }
}

impl Curve {
    /// Build from points (sorted by x; duplicate x values are collapsed).
    pub fn new(mut points: Vec<[f32; 2]>) -> Curve {
        points.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
        points.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-7);
        Curve { points }
    }

    /// `y = x` over [0, 1].
    pub fn identity() -> Curve {
        Curve {
            points: vec![[0.0, 0.0], [1.0, 1.0]],
        }
    }

    /// A constant curve `y = v` over [0, 1].
    pub fn flat(v: f32) -> Curve {
        Curve {
            points: vec![[0.0, v], [1.0, v]],
        }
    }

    /// True when the curve is the identity within `eps`.
    pub fn is_identity(&self, eps: f32) -> bool {
        self.points.iter().all(|p| (p[0] - p[1]).abs() < eps)
    }

    /// True when the curve is the constant `v` within `eps`.
    pub fn is_flat(&self, v: f32, eps: f32) -> bool {
        self.points.iter().all(|p| (p[1] - v).abs() < eps)
    }

    /// Fritsch–Carlson tangents at every control point.
    fn tangents(&self) -> Vec<f32> {
        let p = &self.points;
        let n = p.len();
        if n < 2 {
            return vec![0.0; n];
        }
        let d: Vec<f32> = (0..n - 1)
            .map(|i| {
                let h = p[i + 1][0] - p[i][0];
                if h.abs() < 1e-12 {
                    0.0
                } else {
                    (p[i + 1][1] - p[i][1]) / h
                }
            })
            .collect();
        let mut m = vec![0.0f32; n];
        m[0] = d[0];
        m[n - 1] = d[n - 2];
        for i in 1..n - 1 {
            m[i] = if d[i - 1] * d[i] <= 0.0 {
                0.0
            } else {
                0.5 * (d[i - 1] + d[i])
            };
        }
        for i in 0..n - 1 {
            if d[i].abs() < 1e-12 {
                m[i] = 0.0;
                m[i + 1] = 0.0;
            } else {
                let a = m[i] / d[i];
                let b = m[i + 1] / d[i];
                let s = a * a + b * b;
                if s > 9.0 {
                    let t = 3.0 / s.sqrt();
                    m[i] = t * a * d[i];
                    m[i + 1] = t * b * d[i];
                }
            }
        }
        m
    }

    /// Evaluate at `x`.
    pub fn eval(&self, x: f32) -> f32 {
        let p = &self.points;
        match p.len() {
            0 => x,
            1 => p[0][1],
            _ => {
                let m = self.tangents();
                self.eval_with_tangents(x, &m)
            }
        }
    }

    fn eval_with_tangents(&self, x: f32, m: &[f32]) -> f32 {
        let p = &self.points;
        let n = p.len();
        if x <= p[0][0] {
            return p[0][1] + m[0] * (x - p[0][0]);
        }
        if x >= p[n - 1][0] {
            return p[n - 1][1] + m[n - 1] * (x - p[n - 1][0]);
        }
        // binary search for the segment
        let mut lo = 0;
        let mut hi = n - 1;
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if p[mid][0] <= x {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let h = p[hi][0] - p[lo][0];
        if h.abs() < 1e-12 {
            return p[lo][1];
        }
        let t = (x - p[lo][0]) / h;
        let t2 = t * t;
        let t3 = t2 * t;
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;
        h00 * p[lo][1] + h10 * h * m[lo] + h01 * p[hi][1] + h11 * h * m[hi]
    }

    /// Evaluate at many x values (tangents computed once).
    pub fn eval_many(&self, xs: &[f32]) -> Vec<f32> {
        if self.points.len() < 2 {
            return xs.iter().map(|x| self.eval(*x)).collect();
        }
        let m = self.tangents();
        xs.iter().map(|x| self.eval_with_tangents(*x, &m)).collect()
    }

    /// Bake into a 1D LUT over [0, 1].
    pub fn lut1d(&self, size: usize) -> Lut1D {
        let n = size.max(2);
        let xs: Vec<f32> = (0..n).map(|i| i as f32 / (n - 1) as f32).collect();
        let ys = self.eval_many(&xs);
        let mut l = Lut1D::from_fn(n, |_| [0.0; 3]);
        for (i, y) in ys.iter().enumerate() {
            l.data[i] = [*y, *y, *y];
        }
        l.title = "curve".into();
        l
    }
}

/// The full grading curve set.
///
/// * `master`, `r`, `g`, `b`: value in → value out (identity `y = x`).
/// * `hue_vs_hue`: hue (0–1 turns) in → hue **offset** in turns (identity `y = 0`).
/// * `hue_vs_sat`: hue in → saturation multiplier (identity `y = 1`).
/// * `lum_vs_sat`: luma in → saturation multiplier (identity `y = 1`).
/// * `sat_vs_sat`: saturation in → saturation out (identity `y = x`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Curves {
    pub master: Option<Curve>,
    pub r: Option<Curve>,
    pub g: Option<Curve>,
    pub b: Option<Curve>,
    pub hue_vs_hue: Option<Curve>,
    pub hue_vs_sat: Option<Curve>,
    pub lum_vs_sat: Option<Curve>,
    pub sat_vs_sat: Option<Curve>,
}

impl Curves {
    /// True when no curve is set.
    pub fn is_empty(&self) -> bool {
        self.master.is_none()
            && self.r.is_none()
            && self.g.is_none()
            && self.b.is_none()
            && self.hue_vs_hue.is_none()
            && self.hue_vs_sat.is_none()
            && self.lum_vs_sat.is_none()
            && self.sat_vs_sat.is_none()
    }

    /// Apply to one pixel.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mut c = rgb;
        if let Some(m) = &self.master {
            c = [m.eval(c[0]), m.eval(c[1]), m.eval(c[2])];
        }
        if let Some(r) = &self.r {
            c[0] = r.eval(c[0]);
        }
        if let Some(g) = &self.g {
            c[1] = g.eval(c[1]);
        }
        if let Some(b) = &self.b {
            c[2] = b.eval(c[2]);
        }
        if self.hue_vs_hue.is_none()
            && self.hue_vs_sat.is_none()
            && self.lum_vs_sat.is_none()
            && self.sat_vs_sat.is_none()
        {
            return c;
        }
        let (h, s, v) = rgb_to_hsv(c);
        let luma = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let mut h2 = h;
        let mut s2 = s;
        if let Some(cv) = &self.hue_vs_hue {
            h2 = (h + cv.eval(h)).rem_euclid(1.0);
        }
        if let Some(cv) = &self.hue_vs_sat {
            s2 *= cv.eval(h).max(0.0);
        }
        if let Some(cv) = &self.lum_vs_sat {
            s2 *= cv.eval(luma).max(0.0);
        }
        if let Some(cv) = &self.sat_vs_sat {
            s2 = cv.eval(s2).max(0.0);
        }
        hsv_to_rgb(h2, s2.min(1.0), v)
    }
}

/// RGB → (hue in turns, HSV saturation, value).
pub fn rgb_to_hsv(c: [f32; 3]) -> (f32, f32, f32) {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let delta = max - min;
    let v = max;
    let s = if max.abs() < 1e-12 { 0.0 } else { delta / max };
    let h = if delta.abs() < 1e-12 {
        0.0
    } else if max == c[0] {
        ((c[1] - c[2]) / delta).rem_euclid(6.0) / 6.0
    } else if max == c[1] {
        ((c[2] - c[0]) / delta + 2.0) / 6.0
    } else {
        ((c[0] - c[1]) / delta + 4.0) / 6.0
    };
    (h, s, v)
}

/// (hue in turns, saturation, value) → RGB.
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h6 = h.rem_euclid(1.0) * 6.0;
    let i = h6.floor();
    let f = h6 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i as i32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_through_control_points() {
        let c = Curve::new(vec![[0.0, 0.0], [0.25, 0.1], [0.5, 0.6], [1.0, 1.0]]);
        for p in &c.points {
            assert!((c.eval(p[0]) - p[1]).abs() < 1e-6);
        }
    }

    #[test]
    fn monotone_between_monotone_points() {
        let c = Curve::new(vec![
            [0.0, 0.0],
            [0.2, 0.05],
            [0.4, 0.5],
            [0.6, 0.55],
            [1.0, 1.0],
        ]);
        let mut last = -1.0f32;
        for i in 0..=1000 {
            let y = c.eval(i as f32 / 1000.0);
            assert!(y >= last - 1e-6, "not monotone at {i}");
            last = y;
        }
    }

    #[test]
    fn identity_and_lut() {
        let c = Curve::identity();
        assert!((c.eval(0.3) - 0.3).abs() < 1e-6);
        assert!(c.is_identity(1e-6));
        let l = c.lut1d(16);
        assert!((l.apply([0.5, 0.5, 0.5])[0] - 0.5).abs() < 1e-6);
        // extrapolation
        assert!((c.eval(1.5) - 1.5).abs() < 1e-6);
    }

    #[test]
    fn hsv_round_trip() {
        for c in [
            [0.2, 0.6, 0.1],
            [0.9, 0.1, 0.5],
            [0.3, 0.3, 0.3],
            [0.0, 0.0, 1.0],
        ] {
            let (h, s, v) = rgb_to_hsv(c);
            let back = hsv_to_rgb(h, s, v);
            for i in 0..3 {
                assert!((back[i] - c[i]).abs() < 1e-6, "{c:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn hue_curves() {
        let cs = Curves {
            hue_vs_sat: Some(Curve::new(vec![
                [0.0, 1.0],
                [0.3, 0.0],
                [0.36, 0.0],
                [0.4, 1.0],
                [1.0, 1.0],
            ])),
            ..Default::default()
        };
        let green = cs.apply([0.1, 0.8, 0.1]);
        assert!(
            (green[0] - green[1]).abs() < 1e-3,
            "green desaturated: {green:?}"
        );
        let red = cs.apply([0.8, 0.1, 0.1]);
        assert!((red[0] - 0.8).abs() < 1e-3 && (red[1] - 0.1).abs() < 1e-3);
    }
}
