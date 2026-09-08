//! 1D and 3D lookup tables, the `.cube` file format, and LUT baking.
//!
//! 3D lattices are stored in `.cube` order: red varies fastest, then green,
//! then blue (`index = r + size * (g + size * b)`).

use crate::error::{Error, Result};
use crate::processor::Processor;
use serde::{Deserialize, Serialize};

/// Interpolation used when sampling a [`Lut3D`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    /// Eight-corner trilinear blend.
    Trilinear,
    /// Tetrahedral (default; exact on the lattice diagonals, sharper).
    #[default]
    Tetrahedral,
}

/// A 1D LUT with one or three channels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lut1D {
    /// Human-readable title (from `TITLE`).
    pub title: String,
    /// Number of entries.
    pub size: usize,
    /// Input domain per channel.
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    /// `size` entries, each an RGB triple (a single-channel LUT repeats the value).
    pub data: Vec<[f32; 3]>,
}

impl Lut1D {
    /// Identity LUT of `size` entries over [0, 1].
    pub fn identity(size: usize) -> Lut1D {
        let n = size.max(2);
        let data = (0..n)
            .map(|i| {
                let v = i as f32 / (n - 1) as f32;
                [v, v, v]
            })
            .collect();
        Lut1D {
            title: "identity".into(),
            size: n,
            domain_min: [0.0; 3],
            domain_max: [1.0; 3],
            data,
        }
    }

    /// Build from a per-channel function sampled over [0, 1].
    pub fn from_fn(size: usize, f: impl Fn(f32) -> [f32; 3]) -> Lut1D {
        let n = size.max(2);
        let data = (0..n).map(|i| f(i as f32 / (n - 1) as f32)).collect();
        Lut1D {
            title: "baked".into(),
            size: n,
            domain_min: [0.0; 3],
            domain_max: [1.0; 3],
            data,
        }
    }

    /// Sample one channel with linear interpolation (clamped at the ends).
    pub fn sample(&self, channel: usize, x: f32) -> f32 {
        let lo = self.domain_min[channel];
        let hi = self.domain_max[channel];
        let span = (hi - lo).max(1e-12);
        let t = ((x - lo) / span).clamp(0.0, 1.0) * (self.size - 1) as f32;
        let i = (t.floor() as usize).min(self.size - 2);
        let f = t - i as f32;
        let a = self.data[i][channel];
        let b = self.data[i + 1][channel];
        a + (b - a) * f
    }

    /// Apply to an RGB triple.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        [
            self.sample(0, rgb[0]),
            self.sample(1, rgb[1]),
            self.sample(2, rgb[2]),
        ]
    }

    /// Serialize as `.cube` text.
    pub fn to_cube(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("TITLE \"{}\"\n", self.title.replace('"', "'")));
        s.push_str(&format!("LUT_1D_SIZE {}\n", self.size));
        s.push_str(&format!(
            "DOMAIN_MIN {} {} {}\nDOMAIN_MAX {} {} {}\n",
            self.domain_min[0],
            self.domain_min[1],
            self.domain_min[2],
            self.domain_max[0],
            self.domain_max[1],
            self.domain_max[2]
        ));
        for e in &self.data {
            s.push_str(&format!("{:.6} {:.6} {:.6}\n", e[0], e[1], e[2]));
        }
        s
    }
}

/// A 3D LUT lattice.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lut3D {
    pub title: String,
    /// Lattice edge length.
    pub size: usize,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    /// `size³` RGB entries, red fastest.
    pub data: Vec<[f32; 3]>,
    /// Interpolation used by [`Lut3D::apply`].
    pub interpolation: Interpolation,
}

impl Lut3D {
    /// Identity lattice.
    pub fn identity(size: usize) -> Lut3D {
        Self::from_fn(size, |rgb| rgb)
    }

    /// Build a lattice by evaluating `f` at every node over [0, 1]³.
    pub fn from_fn(size: usize, f: impl Fn([f32; 3]) -> [f32; 3]) -> Lut3D {
        let n = size.max(2);
        let mut data = Vec::with_capacity(n * n * n);
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let d = (n - 1) as f32;
                    data.push(f([r as f32 / d, g as f32 / d, b as f32 / d]));
                }
            }
        }
        Lut3D {
            title: "baked".into(),
            size: n,
            domain_min: [0.0; 3],
            domain_max: [1.0; 3],
            data,
            interpolation: Interpolation::Tetrahedral,
        }
    }

    /// Bake a processor into a lattice of `size` over the unit cube.
    pub fn bake(processor: &Processor, size: usize) -> Lut3D {
        let mut l = Self::from_fn(size, |rgb| processor.apply_rgb(&rgb));
        l.title = "baked processor".into();
        l
    }

    /// Bake over an arbitrary input domain (e.g. log encodings covering [-0.1, 1.1]).
    pub fn bake_domain(
        processor: &Processor,
        size: usize,
        domain_min: [f32; 3],
        domain_max: [f32; 3],
    ) -> Lut3D {
        let mut l = Self::from_fn(size, |t| {
            let x = [
                domain_min[0] + t[0] * (domain_max[0] - domain_min[0]),
                domain_min[1] + t[1] * (domain_max[1] - domain_min[1]),
                domain_min[2] + t[2] * (domain_max[2] - domain_min[2]),
            ];
            processor.apply_rgb(&x)
        });
        l.domain_min = domain_min;
        l.domain_max = domain_max;
        l
    }

    #[inline]
    fn node(&self, r: usize, g: usize, b: usize) -> [f32; 3] {
        self.data[r + self.size * (g + self.size * b)]
    }

    /// Apply with the configured interpolation.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        self.apply_with(rgb, self.interpolation)
    }

    /// Apply with an explicit interpolation mode.
    pub fn apply_with(&self, rgb: [f32; 3], interp: Interpolation) -> [f32; 3] {
        let n = self.size;
        let mut idx = [0usize; 3];
        let mut frac = [0f32; 3];
        for c in 0..3 {
            let span = (self.domain_max[c] - self.domain_min[c]).max(1e-12);
            let t = ((rgb[c] - self.domain_min[c]) / span).clamp(0.0, 1.0) * (n - 1) as f32;
            let i = (t.floor() as usize).min(n - 2);
            idx[c] = i;
            frac[c] = t - i as f32;
        }
        let (r, g, b) = (idx[0], idx[1], idx[2]);
        let (fr, fg, fb) = (frac[0], frac[1], frac[2]);
        match interp {
            Interpolation::Trilinear => {
                let c000 = self.node(r, g, b);
                let c100 = self.node(r + 1, g, b);
                let c010 = self.node(r, g + 1, b);
                let c110 = self.node(r + 1, g + 1, b);
                let c001 = self.node(r, g, b + 1);
                let c101 = self.node(r + 1, g, b + 1);
                let c011 = self.node(r, g + 1, b + 1);
                let c111 = self.node(r + 1, g + 1, b + 1);
                let mut out = [0f32; 3];
                for c in 0..3 {
                    let c00 = c000[c] + (c100[c] - c000[c]) * fr;
                    let c10 = c010[c] + (c110[c] - c010[c]) * fr;
                    let c01 = c001[c] + (c101[c] - c001[c]) * fr;
                    let c11 = c011[c] + (c111[c] - c011[c]) * fr;
                    let c0 = c00 + (c10 - c00) * fg;
                    let c1 = c01 + (c11 - c01) * fg;
                    out[c] = c0 + (c1 - c0) * fb;
                }
                out
            }
            Interpolation::Tetrahedral => {
                let c000 = self.node(r, g, b);
                let c111 = self.node(r + 1, g + 1, b + 1);
                let (w0, wa, wb, w3, ca, cb);
                if fr >= fg {
                    if fg >= fb {
                        // r >= g >= b
                        ca = self.node(r + 1, g, b);
                        cb = self.node(r + 1, g + 1, b);
                        w0 = 1.0 - fr;
                        wa = fr - fg;
                        wb = fg - fb;
                        w3 = fb;
                    } else if fr >= fb {
                        // r >= b > g
                        ca = self.node(r + 1, g, b);
                        cb = self.node(r + 1, g, b + 1);
                        w0 = 1.0 - fr;
                        wa = fr - fb;
                        wb = fb - fg;
                        w3 = fg;
                    } else {
                        // b > r >= g
                        ca = self.node(r, g, b + 1);
                        cb = self.node(r + 1, g, b + 1);
                        w0 = 1.0 - fb;
                        wa = fb - fr;
                        wb = fr - fg;
                        w3 = fg;
                    }
                } else if fb >= fg {
                    // b >= g > r
                    ca = self.node(r, g, b + 1);
                    cb = self.node(r, g + 1, b + 1);
                    w0 = 1.0 - fb;
                    wa = fb - fg;
                    wb = fg - fr;
                    w3 = fr;
                } else if fb >= fr {
                    // g > b >= r
                    ca = self.node(r, g + 1, b);
                    cb = self.node(r, g + 1, b + 1);
                    w0 = 1.0 - fg;
                    wa = fg - fb;
                    wb = fb - fr;
                    w3 = fr;
                } else {
                    // g > r > b
                    ca = self.node(r, g + 1, b);
                    cb = self.node(r + 1, g + 1, b);
                    w0 = 1.0 - fg;
                    wa = fg - fr;
                    wb = fr - fb;
                    w3 = fb;
                }
                [
                    w0 * c000[0] + wa * ca[0] + wb * cb[0] + w3 * c111[0],
                    w0 * c000[1] + wa * ca[1] + wb * cb[1] + w3 * c111[1],
                    w0 * c000[2] + wa * ca[2] + wb * cb[2] + w3 * c111[2],
                ]
            }
        }
    }

    /// Serialize as `.cube` text.
    pub fn to_cube(&self) -> String {
        let mut s = String::with_capacity(self.data.len() * 30);
        s.push_str(&format!("TITLE \"{}\"\n", self.title.replace('"', "'")));
        s.push_str(&format!("LUT_3D_SIZE {}\n", self.size));
        s.push_str(&format!(
            "DOMAIN_MIN {} {} {}\nDOMAIN_MAX {} {} {}\n",
            self.domain_min[0],
            self.domain_min[1],
            self.domain_min[2],
            self.domain_max[0],
            self.domain_max[1],
            self.domain_max[2]
        ));
        for e in &self.data {
            s.push_str(&format!("{:.6} {:.6} {:.6}\n", e[0], e[1], e[2]));
        }
        s
    }
}

/// Either kind of LUT, as read from a `.cube` file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Lut {
    OneD(Lut1D),
    ThreeD(Lut3D),
}

impl Lut {
    /// Apply whichever LUT this is.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        match self {
            Lut::OneD(l) => l.apply(rgb),
            Lut::ThreeD(l) => l.apply(rgb),
        }
    }

    /// Serialize as `.cube` text.
    pub fn to_cube(&self) -> String {
        match self {
            Lut::OneD(l) => l.to_cube(),
            Lut::ThreeD(l) => l.to_cube(),
        }
    }

    /// The 3D LUT, if that is what was loaded.
    pub fn as_3d(&self) -> Option<&Lut3D> {
        match self {
            Lut::ThreeD(l) => Some(l),
            _ => None,
        }
    }
}

fn parse_floats(rest: &str, n: usize, line: usize) -> Result<Vec<f32>> {
    let v: std::result::Result<Vec<f32>, _> = rest
        .split_whitespace()
        .take(n)
        .map(|t| t.parse::<f32>())
        .collect();
    match v {
        Ok(v) if v.len() == n => Ok(v),
        _ => Err(Error::Parse {
            line,
            message: format!("expected {n} numbers, got `{rest}`"),
        }),
    }
}

/// Parse Adobe/Resolve `.cube` text (1D or 3D).
pub fn parse_cube(text: &str) -> Result<Lut> {
    let mut title = String::new();
    let mut size_1d: Option<usize> = None;
    let mut size_3d: Option<usize> = None;
    let mut dmin = [0f32; 3];
    let mut dmax = [1f32; 3];
    let mut data: Vec<[f32; 3]> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let ln = i + 1;
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (kw, rest) = match line.split_once(char::is_whitespace) {
            Some((k, r)) => (k, r.trim()),
            None => (line, ""),
        };
        match kw.to_ascii_uppercase().as_str() {
            "TITLE" => title = rest.trim_matches('"').to_string(),
            "LUT_1D_SIZE" => {
                size_1d = Some(rest.parse().map_err(|_| Error::Parse {
                    line: ln,
                    message: "bad LUT_1D_SIZE".into(),
                })?)
            }
            "LUT_3D_SIZE" => {
                size_3d = Some(rest.parse().map_err(|_| Error::Parse {
                    line: ln,
                    message: "bad LUT_3D_SIZE".into(),
                })?)
            }
            "DOMAIN_MIN" => {
                let v = parse_floats(rest, 3, ln)?;
                dmin = [v[0], v[1], v[2]];
            }
            "DOMAIN_MAX" => {
                let v = parse_floats(rest, 3, ln)?;
                dmax = [v[0], v[1], v[2]];
            }
            "LUT_1D_INPUT_RANGE" | "LUT_3D_INPUT_RANGE" => {
                let v = parse_floats(rest, 2, ln)?;
                dmin = [v[0]; 3];
                dmax = [v[1]; 3];
            }
            _ => {
                if kw.parse::<f32>().is_ok() {
                    let v = parse_floats(line, 3, ln)?;
                    data.push([v[0], v[1], v[2]]);
                } else {
                    // Unknown keyword: ignored for forward compatibility.
                }
            }
        }
    }
    match (size_1d, size_3d) {
        (Some(_), Some(_)) => Err(Error::Lut(
            "both LUT_1D_SIZE and LUT_3D_SIZE present".into(),
        )),
        (None, None) => Err(Error::Lut("missing LUT_1D_SIZE / LUT_3D_SIZE".into())),
        (Some(n), None) => {
            if n < 2 || data.len() != n {
                return Err(Error::Lut(format!(
                    "1D LUT expects {n} entries, found {}",
                    data.len()
                )));
            }
            Ok(Lut::OneD(Lut1D {
                title,
                size: n,
                domain_min: dmin,
                domain_max: dmax,
                data,
            }))
        }
        (None, Some(n)) => {
            if n < 2 || data.len() != n * n * n {
                return Err(Error::Lut(format!(
                    "3D LUT expects {} entries, found {}",
                    n * n * n,
                    data.len()
                )));
            }
            Ok(Lut::ThreeD(Lut3D {
                title,
                size: n,
                domain_min: dmin,
                domain_max: dmax,
                data,
                interpolation: Interpolation::Tetrahedral,
            }))
        }
    }
}

/// Read and parse a `.cube` file from disk.
pub fn load_cube(path: &std::path::Path) -> Result<Lut> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    parse_cube(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_passes_through() {
        let l = Lut3D::identity(17);
        for &v in &[
            [0.0, 0.0, 0.0],
            [0.1, 0.5, 0.9],
            [0.333, 0.777, 0.123],
            [1.0, 1.0, 1.0],
        ] {
            for interp in [Interpolation::Tetrahedral, Interpolation::Trilinear] {
                let o = l.apply_with(v, interp);
                for c in 0..3 {
                    assert!((o[c] - v[c]).abs() < 1e-6, "{interp:?} {v:?} -> {o:?}");
                }
            }
        }
    }

    #[test]
    fn interpolators_agree_on_lattice_points() {
        let l = Lut3D::from_fn(9, |c| {
            [c[0].powf(1.8), c[1] * 0.8 + 0.1, (c[2] * c[0]).sqrt()]
        });
        for b in 0..9 {
            for g in 0..9 {
                for r in 0..9 {
                    let v = [r as f32 / 8.0, g as f32 / 8.0, b as f32 / 8.0];
                    let a = l.apply_with(v, Interpolation::Tetrahedral);
                    let t = l.apply_with(v, Interpolation::Trilinear);
                    for c in 0..3 {
                        assert!((a[c] - t[c]).abs() < 1e-6);
                        assert!((a[c] - l.node(r, g, b)[c]).abs() < 1e-6);
                    }
                }
            }
        }
    }

    #[test]
    fn interpolators_close_off_lattice() {
        let l = Lut3D::from_fn(17, |c| {
            [c[0].powf(1.8), c[1] * 0.8 + 0.1, (c[2] * c[0]).sqrt()]
        });
        let v = [0.31, 0.62, 0.17];
        let a = l.apply_with(v, Interpolation::Tetrahedral);
        let t = l.apply_with(v, Interpolation::Trilinear);
        for c in 0..3 {
            assert!((a[c] - t[c]).abs() < 5e-3, "{a:?} vs {t:?}");
        }
    }

    #[test]
    fn cube_round_trip() {
        let l = Lut3D::from_fn(5, |c| [c[0] * 0.9, c[1], c[2] + 0.05]);
        let text = l.to_cube();
        let back = parse_cube(&text).unwrap();
        let b = back.as_3d().unwrap();
        assert_eq!(b.size, 5);
        assert_eq!(b.data.len(), 125);
        assert!((b.apply([0.5, 0.5, 0.5])[2] - 0.55).abs() < 1e-5);
        let one = Lut1D::from_fn(64, |x| [x * x, x, x.sqrt()]);
        let back = parse_cube(&one.to_cube()).unwrap();
        match back {
            Lut::OneD(l) => assert!((l.apply([0.5, 0.5, 0.25])[0] - 0.25).abs() < 1e-3),
            _ => panic!("expected 1D"),
        }
    }

    #[test]
    fn bad_cube_errors() {
        assert!(parse_cube("LUT_3D_SIZE 2\n0 0 0\n").is_err());
        assert!(parse_cube("0 0 0\n").is_err());
    }
}
