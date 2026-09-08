//! Colorimetry: chromaticities, RGB↔XYZ matrices, chromatic adaptation and
//! the [`ColorSpace`] description used to build processors.
//!
//! All matrices are row-major `[[f64; 3]; 3]` and act on column vectors:
//! `out = M * in`.

#![allow(clippy::needless_range_loop)]

use crate::transfer::Transfer;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Mat3
// ---------------------------------------------------------------------------

/// A 3×3 row-major matrix.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mat3(pub [[f64; 3]; 3]);

impl Mat3 {
    /// The identity matrix.
    pub const IDENTITY: Mat3 = Mat3([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    /// Diagonal matrix.
    pub fn diag(d: [f64; 3]) -> Mat3 {
        Mat3([[d[0], 0.0, 0.0], [0.0, d[1], 0.0], [0.0, 0.0, d[2]]])
    }

    /// Build from three column vectors.
    pub fn from_cols(c0: [f64; 3], c1: [f64; 3], c2: [f64; 3]) -> Mat3 {
        Mat3([
            [c0[0], c1[0], c2[0]],
            [c0[1], c1[1], c2[1]],
            [c0[2], c1[2], c2[2]],
        ])
    }

    /// Row-major flat 9-element array.
    pub fn from_flat(f: [f64; 9]) -> Mat3 {
        Mat3([[f[0], f[1], f[2]], [f[3], f[4], f[5]], [f[6], f[7], f[8]]])
    }

    /// Flatten row-major.
    pub fn to_flat(&self) -> [f64; 9] {
        let m = self.0;
        [
            m[0][0], m[0][1], m[0][2], m[1][0], m[1][1], m[1][2], m[2][0], m[2][1], m[2][2],
        ]
    }

    /// `self * other` (apply `other` first, then `self`).
    pub fn mul(&self, other: &Mat3) -> Mat3 {
        let a = self.0;
        let b = other.0;
        let mut r = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                r[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
            }
        }
        Mat3(r)
    }

    /// Matrix–vector product.
    pub fn apply(&self, v: [f64; 3]) -> [f64; 3] {
        let m = self.0;
        [
            m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
            m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
            m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
        ]
    }

    /// Transposed copy.
    pub fn transpose(&self) -> Mat3 {
        let m = self.0;
        Mat3([
            [m[0][0], m[1][0], m[2][0]],
            [m[0][1], m[1][1], m[2][1]],
            [m[0][2], m[1][2], m[2][2]],
        ])
    }

    /// Determinant.
    pub fn det(&self) -> f64 {
        let m = self.0;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    /// Inverse, or `None` when singular.
    pub fn inverse(&self) -> Option<Mat3> {
        let m = self.0;
        let det = self.det();
        if det.abs() < 1e-18 {
            return None;
        }
        let inv = 1.0 / det;
        Some(Mat3([
            [
                (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv,
                (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv,
                (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv,
            ],
            [
                (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv,
                (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv,
                (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv,
            ],
            [
                (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv,
                (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv,
                (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv,
            ],
        ]))
    }

    /// True when every element is within `eps` of the identity.
    pub fn is_identity(&self, eps: f64) -> bool {
        for i in 0..3 {
            for j in 0..3 {
                let want = if i == j { 1.0 } else { 0.0 };
                if (self.0[i][j] - want).abs() > eps {
                    return false;
                }
            }
        }
        true
    }

    /// Max absolute element difference.
    pub fn max_abs_diff(&self, other: &Mat3) -> f64 {
        let mut d: f64 = 0.0;
        for i in 0..3 {
            for j in 0..3 {
                d = d.max((self.0[i][j] - other.0[i][j]).abs());
            }
        }
        d
    }
}

// ---------------------------------------------------------------------------
// Primaries
// ---------------------------------------------------------------------------

/// CIE xy chromaticities of the three primaries and the white point.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Primaries {
    pub r: [f64; 2],
    pub g: [f64; 2],
    pub b: [f64; 2],
    pub w: [f64; 2],
}

/// CIE D65 white (Rec.709 / sRGB / Rec.2020).
pub const WHITE_D65: [f64; 2] = [0.3127, 0.3290];
/// ACES white point (≈ D60).
pub const WHITE_ACES: [f64; 2] = [0.32168, 0.33767];
/// DCI theatrical white.
pub const WHITE_DCI: [f64; 2] = [0.314, 0.351];
/// CIE D50 white (ICC PCS).
pub const WHITE_D50: [f64; 2] = [0.3457, 0.3585];

impl Primaries {
    /// ITU-R BT.709 / sRGB.
    pub const REC709: Primaries = Primaries {
        r: [0.64, 0.33],
        g: [0.30, 0.60],
        b: [0.15, 0.06],
        w: WHITE_D65,
    };
    /// ITU-R BT.2020 / BT.2100.
    pub const REC2020: Primaries = Primaries {
        r: [0.708, 0.292],
        g: [0.170, 0.797],
        b: [0.131, 0.046],
        w: WHITE_D65,
    };
    /// DCI-P3 (theatrical, DCI white).
    pub const DCI_P3: Primaries = Primaries {
        r: [0.680, 0.320],
        g: [0.265, 0.690],
        b: [0.150, 0.060],
        w: WHITE_DCI,
    };
    /// Display P3 / P3-D65.
    pub const DISPLAY_P3: Primaries = Primaries {
        r: [0.680, 0.320],
        g: [0.265, 0.690],
        b: [0.150, 0.060],
        w: WHITE_D65,
    };
    /// ACES AP0 (ACES2065-1).
    pub const ACES_AP0: Primaries = Primaries {
        r: [0.7347, 0.2653],
        g: [0.0, 1.0],
        b: [0.0001, -0.077],
        w: WHITE_ACES,
    };
    /// ACES AP1 (ACEScg / ACEScc / ACEScct).
    pub const ACES_AP1: Primaries = Primaries {
        r: [0.713, 0.293],
        g: [0.165, 0.830],
        b: [0.128, 0.044],
        w: WHITE_ACES,
    };
    /// ARRI Wide Gamut 3 (ALEXA classic).
    pub const ARRI_WIDE_GAMUT3: Primaries = Primaries {
        r: [0.6840, 0.3130],
        g: [0.2210, 0.8480],
        b: [0.0861, -0.1020],
        w: WHITE_D65,
    };
    /// ARRI Wide Gamut 4 (ALEXA 35).
    pub const ARRI_WIDE_GAMUT4: Primaries = Primaries {
        r: [0.7347, 0.2653],
        g: [0.1424, 0.8576],
        b: [0.0991, -0.0308],
        w: WHITE_D65,
    };
    /// Sony S-Gamut3.
    pub const S_GAMUT3: Primaries = Primaries {
        r: [0.730, 0.280],
        g: [0.140, 0.855],
        b: [0.100, -0.050],
        w: WHITE_D65,
    };
    /// Sony S-Gamut3.Cine.
    pub const S_GAMUT3_CINE: Primaries = Primaries {
        r: [0.766, 0.275],
        g: [0.225, 0.800],
        b: [0.089, -0.087],
        w: WHITE_D65,
    };
    /// RED Wide Gamut RGB.
    pub const RED_WIDE_GAMUT: Primaries = Primaries {
        r: [0.780308, 0.304253],
        g: [0.121595, 1.493994],
        b: [0.095612, -0.084589],
        w: WHITE_D65,
    };
    /// Panasonic V-Gamut.
    pub const V_GAMUT: Primaries = Primaries {
        r: [0.730, 0.280],
        g: [0.165, 0.840],
        b: [0.100, -0.030],
        w: WHITE_D65,
    };
    /// Canon Cinema Gamut.
    pub const CANON_CINEMA_GAMUT: Primaries = Primaries {
        r: [0.74, 0.27],
        g: [0.17, 1.14],
        b: [0.08, -0.10],
        w: WHITE_D65,
    };
    /// CIE XYZ expressed as an RGB space (identity primaries, D65 white) — used for display references.
    pub const XYZ_D65: Primaries = Primaries {
        r: [1.0, 0.0],
        g: [0.0, 1.0],
        b: [0.0, 0.0],
        w: WHITE_D65,
    };

    /// RGB → CIE XYZ matrix (white maps to Y = 1).
    pub fn rgb_to_xyz(&self) -> Mat3 {
        if self.b[1].abs() < 1e-12 {
            // XYZ-as-RGB (the Z axis has no finite chromaticity): identity by definition.
            return Mat3::IDENTITY;
        }
        let m = Mat3::from_cols(xy_to_xyz(self.r), xy_to_xyz(self.g), xy_to_xyz(self.b));
        let w = xy_to_xyz(self.w);
        let s = m.inverse().unwrap_or(Mat3::IDENTITY).apply(w);
        m.mul(&Mat3::diag(s))
    }

    /// CIE XYZ → RGB matrix.
    pub fn xyz_to_rgb(&self) -> Mat3 {
        self.rgb_to_xyz().inverse().unwrap_or(Mat3::IDENTITY)
    }

    /// Luminance weights of the primaries (the Y row of [`Self::rgb_to_xyz`]).
    pub fn luma_weights(&self) -> [f64; 3] {
        self.rgb_to_xyz().0[1]
    }

    /// True when both chromaticity sets are equal within `eps`.
    pub fn approx_eq(&self, other: &Primaries, eps: f64) -> bool {
        let d = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < eps && (a[1] - b[1]).abs() < eps;
        d(self.r, other.r) && d(self.g, other.g) && d(self.b, other.b) && d(self.w, other.w)
    }
}

/// Serializable name for a well-known set of primaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimariesId {
    Rec709,
    Rec2020,
    DciP3,
    DisplayP3,
    AcesAp0,
    AcesAp1,
    ArriWideGamut3,
    ArriWideGamut4,
    SGamut3,
    SGamut3Cine,
    RedWideGamut,
    VGamut,
    CanonCinemaGamut,
    XyzD65,
}

impl PrimariesId {
    /// The chromaticities.
    pub fn primaries(&self) -> Primaries {
        match self {
            PrimariesId::Rec709 => Primaries::REC709,
            PrimariesId::Rec2020 => Primaries::REC2020,
            PrimariesId::DciP3 => Primaries::DCI_P3,
            PrimariesId::DisplayP3 => Primaries::DISPLAY_P3,
            PrimariesId::AcesAp0 => Primaries::ACES_AP0,
            PrimariesId::AcesAp1 => Primaries::ACES_AP1,
            PrimariesId::ArriWideGamut3 => Primaries::ARRI_WIDE_GAMUT3,
            PrimariesId::ArriWideGamut4 => Primaries::ARRI_WIDE_GAMUT4,
            PrimariesId::SGamut3 => Primaries::S_GAMUT3,
            PrimariesId::SGamut3Cine => Primaries::S_GAMUT3_CINE,
            PrimariesId::RedWideGamut => Primaries::RED_WIDE_GAMUT,
            PrimariesId::VGamut => Primaries::V_GAMUT,
            PrimariesId::CanonCinemaGamut => Primaries::CANON_CINEMA_GAMUT,
            PrimariesId::XyzD65 => Primaries::XYZ_D65,
        }
    }
}

/// xy chromaticity → XYZ with Y = 1.
pub fn xy_to_xyz(xy: [f64; 2]) -> [f64; 3] {
    let (x, y) = (xy[0], xy[1]);
    if y.abs() < 1e-12 {
        return [0.0, 0.0, 0.0];
    }
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// XYZ → xy chromaticity.
pub fn xyz_to_xy(xyz: [f64; 3]) -> [f64; 2] {
    let s = xyz[0] + xyz[1] + xyz[2];
    if s.abs() < 1e-12 {
        return [0.0, 0.0];
    }
    [xyz[0] / s, xyz[1] / s]
}

/// The Bradford cone response matrix.
pub const BRADFORD: Mat3 = Mat3([
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
]);

/// Bradford chromatic adaptation matrix (XYZ → XYZ) from white `src` to white `dst`.
pub fn bradford_adaptation(src: [f64; 2], dst: [f64; 2]) -> Mat3 {
    if (src[0] - dst[0]).abs() < 1e-9 && (src[1] - dst[1]).abs() < 1e-9 {
        return Mat3::IDENTITY;
    }
    let ws = BRADFORD.apply(xy_to_xyz(src));
    let wd = BRADFORD.apply(xy_to_xyz(dst));
    let scale = Mat3::diag([wd[0] / ws[0], wd[1] / ws[1], wd[2] / ws[2]]);
    let inv = BRADFORD.inverse().unwrap_or(Mat3::IDENTITY);
    inv.mul(&scale).mul(&BRADFORD)
}

/// RGB (src primaries) → RGB (dst primaries) including Bradford adaptation
/// between the two white points.
pub fn rgb_to_rgb_matrix(src: &Primaries, dst: &Primaries) -> Mat3 {
    let cat = bradford_adaptation(src.w, dst.w);
    dst.xyz_to_rgb().mul(&cat).mul(&src.rgb_to_xyz())
}

/// RGB (src primaries) → CIE XYZ adapted to `dst_white`.
pub fn rgb_to_xyz_adapted(src: &Primaries, dst_white: [f64; 2]) -> Mat3 {
    bradford_adaptation(src.w, dst_white).mul(&src.rgb_to_xyz())
}

/// Approximate chromaticity of a CIE daylight illuminant at `cct` kelvin
/// (valid 4000–25000 K; clamped outside).
pub fn daylight_chromaticity(cct: f64) -> [f64; 2] {
    let t = cct.clamp(4000.0, 25000.0);
    let x = if t <= 7000.0 {
        -4.6070e9 / (t * t * t) + 2.9678e6 / (t * t) + 0.09911e3 / t + 0.244063
    } else {
        -2.0064e9 / (t * t * t) + 1.9018e6 / (t * t) + 0.24748e3 / t + 0.23704
    };
    let y = -3.0 * x * x + 2.87 * x - 0.275;
    [x, y]
}

/// Lowest CCT of the Planckian locus approximation.
pub const PLANCKIAN_MIN_K: f64 = 1667.0;
/// Highest CCT of the Planckian locus approximation.
pub const PLANCKIAN_MAX_K: f64 = 25000.0;

/// Chromaticity of a Planckian (black body) radiator at `cct` kelvin
/// (Kim et al. 2002 cubic-spline fit, valid 1667–25000 K; clamped outside).
pub fn planckian_chromaticity(cct: f64) -> [f64; 2] {
    let t = cct.clamp(PLANCKIAN_MIN_K, PLANCKIAN_MAX_K);
    let (t2, t3) = (t * t, t * t * t);
    let x = if t <= 4000.0 {
        -0.2661239e9 / t3 - 0.2343589e6 / t2 + 0.8776956e3 / t + 0.179910
    } else {
        -3.0258469e9 / t3 + 2.1070379e6 / t2 + 0.2226347e3 / t + 0.240390
    };
    let (x2, x3) = (x * x, x * x * x);
    let y = if t <= 2222.0 {
        -1.1063814 * x3 - 1.34811020 * x2 + 2.18555832 * x - 0.20219683
    } else if t <= 4000.0 {
        -0.9549476 * x3 - 1.37418593 * x2 + 2.09137015 * x - 0.16748867
    } else {
        3.0817580 * x3 - 5.87338670 * x2 + 3.75112997 * x - 0.37001483
    };
    [x, y]
}

/// CIE xy → CIE 1960 UCS `uv` (the space in which Duv is measured).
pub fn xy_to_uv(xy: [f64; 2]) -> [f64; 2] {
    let d = -2.0 * xy[0] + 12.0 * xy[1] + 3.0;
    if d.abs() < 1e-12 {
        return [0.0, 0.0];
    }
    [4.0 * xy[0] / d, 6.0 * xy[1] / d]
}

/// CIE 1960 UCS `uv` → CIE xy.
pub fn uv_to_xy(uv: [f64; 2]) -> [f64; 2] {
    let d = 2.0 * uv[0] - 8.0 * uv[1] + 4.0;
    if d.abs() < 1e-12 {
        return [0.0, 0.0];
    }
    [3.0 * uv[0] / d, 2.0 * uv[1] / d]
}

/// Correlated color temperature of a chromaticity (McCamy's cubic
/// approximation, accurate to a few kelvin for daylight-like whites).
pub fn cct_mccamy(xy: [f64; 2]) -> f64 {
    let n = (xy[0] - 0.3320) / (0.1858 - xy[1]);
    449.0 * n * n * n + 3525.0 * n * n + 6823.3 * n + 5520.33
}

/// Unit vector in `uv` perpendicular to the Planckian locus at `cct`,
/// pointing to the "above the locus" side (positive Duv = green).
pub fn planckian_normal_uv(cct: f64) -> [f64; 2] {
    let t = cct.clamp(PLANCKIAN_MIN_K * 1.01, PLANCKIAN_MAX_K / 1.01);
    let a = xy_to_uv(planckian_chromaticity(t * 1.01));
    let b = xy_to_uv(planckian_chromaticity(t / 1.01));
    // tangent toward lower temperatures (increasing u), rotated 90° counter-clockwise
    let tangent = [b[0] - a[0], b[1] - a[1]];
    let len = (tangent[0] * tangent[0] + tangent[1] * tangent[1])
        .sqrt()
        .max(1e-30);
    let n = [-tangent[1] / len, tangent[0] / len];
    if n[1] < 0.0 {
        [-n[0], -n[1]]
    } else {
        n
    }
}

/// Chromaticity at `cct` kelvin on the Planckian locus, displaced by `duv`
/// along the perpendicular (positive = above the locus = green).
pub fn planckian_with_duv(cct: f64, duv: f64) -> [f64; 2] {
    let uv = xy_to_uv(planckian_chromaticity(cct));
    let n = planckian_normal_uv(cct);
    uv_to_xy([uv[0] + duv * n[0], uv[1] + duv * n[1]])
}

// ---------------------------------------------------------------------------
// ColorSpace
// ---------------------------------------------------------------------------

/// A complete colorspace description: primaries + white, transfer function,
/// and whether the encoded values are scene-referred linear light.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorSpace {
    /// Canonical name (e.g. `ACEScg`).
    pub name: String,
    pub primaries: Primaries,
    pub transfer: Transfer,
    /// True when values are scene-linear (no display referred tonescale applied).
    pub is_scene_linear: bool,
    /// Alternative names accepted by lookups.
    pub aliases: Vec<String>,
}

impl ColorSpace {
    /// Build a colorspace.
    pub fn new(name: &str, primaries: Primaries, transfer: Transfer) -> Self {
        ColorSpace {
            name: name.to_string(),
            primaries,
            is_scene_linear: matches!(transfer, Transfer::Linear),
            transfer,
            aliases: Vec::new(),
        }
    }

    /// Add aliases (builder style).
    pub fn with_aliases(mut self, aliases: &[&str]) -> Self {
        self.aliases.extend(aliases.iter().map(|s| s.to_string()));
        self
    }

    /// Mark as display-referred (not scene-linear even if the transfer is linear).
    pub fn display_referred(mut self) -> Self {
        self.is_scene_linear = false;
        self
    }

    /// True when `name` matches the canonical name or an alias (case-insensitive).
    pub fn matches(&self, name: &str) -> bool {
        let n = name.trim();
        self.name.eq_ignore_ascii_case(n) || self.aliases.iter().any(|a| a.eq_ignore_ascii_case(n))
    }

    /// The built-in table of camera, working and display colorspaces.
    pub fn builtins() -> Vec<ColorSpace> {
        use Transfer as T;
        vec![
            ColorSpace::new("ACES2065-1", Primaries::ACES_AP0, T::Linear).with_aliases(&[
                "aces",
                "ACES - ACES2065-1",
                "ap0",
            ]),
            ColorSpace::new("ACEScg", Primaries::ACES_AP1, T::Linear).with_aliases(&[
                "ACES - ACEScg",
                "ap1",
                "lin_ap1",
            ]),
            ColorSpace::new("ACEScct", Primaries::ACES_AP1, T::ACEScct)
                .with_aliases(&["ACES - ACEScct"]),
            ColorSpace::new("ACEScc", Primaries::ACES_AP1, T::ACEScc)
                .with_aliases(&["ACES - ACEScc"]),
            ColorSpace::new("Linear Rec.709", Primaries::REC709, T::Linear).with_aliases(&[
                "lin_rec709",
                "lin_srgb",
                "Linear sRGB",
                "Utility - Linear - sRGB",
            ]),
            ColorSpace::new("sRGB", Primaries::REC709, T::SRgb)
                .with_aliases(&["srgb_texture", "Utility - sRGB - Texture", "sRGB - Display"])
                .display_referred(),
            ColorSpace::new("Rec.709", Primaries::REC709, T::Gamma(2.4))
                .with_aliases(&["rec709", "Rec.1886", "Rec.709 - Display", "rec709_display"])
                .display_referred(),
            ColorSpace::new("Rec.709 Camera", Primaries::REC709, T::Rec709)
                .with_aliases(&["rec709_oetf", "Utility - Rec.709 - Camera"]),
            ColorSpace::new("Linear Rec.2020", Primaries::REC2020, T::Linear)
                .with_aliases(&["lin_rec2020"]),
            ColorSpace::new("Rec.2020", Primaries::REC2020, T::Gamma(2.4))
                .with_aliases(&["rec2020", "Rec.2020 - Display"])
                .display_referred(),
            ColorSpace::new("Rec.2100-PQ", Primaries::REC2020, T::Pq)
                .with_aliases(&["rec2100_pq", "ST2084", "PQ", "Rec.2100-PQ - Display"])
                .display_referred(),
            ColorSpace::new("Rec.2100-HLG", Primaries::REC2020, T::Hlg)
                .with_aliases(&["rec2100_hlg", "HLG", "Rec.2100-HLG - Display"])
                .display_referred(),
            ColorSpace::new("Linear P3-D65", Primaries::DISPLAY_P3, T::Linear)
                .with_aliases(&["lin_p3d65"]),
            ColorSpace::new("Display P3", Primaries::DISPLAY_P3, T::SRgb)
                .with_aliases(&["displayp3", "Display P3 - Display", "P3-D65 - Display"])
                .display_referred(),
            ColorSpace::new("DCI-P3", Primaries::DCI_P3, T::Gamma(2.6))
                .with_aliases(&["p3dci", "DCI-P3 - Display"])
                .display_referred(),
            ColorSpace::new("CIE-XYZ-D65", Primaries::XYZ_D65, T::Linear)
                .with_aliases(&["xyz_d65", "XYZ"]),
            ColorSpace::new("ARRI LogC4", Primaries::ARRI_WIDE_GAMUT4, T::LogC4).with_aliases(&[
                "arri_logc4",
                "ARRI LogC4 AWG4",
                "LogC4",
            ]),
            ColorSpace::new("ARRI LogC3", Primaries::ARRI_WIDE_GAMUT3, T::LogC3).with_aliases(&[
                "arri_logc3_ei800",
                "ARRI LogC3 (EI800)",
                "LogC3",
                "Input - ARRI - V3 LogC (EI800) - Wide Gamut",
            ]),
            ColorSpace::new(
                "Linear ARRI Wide Gamut 4",
                Primaries::ARRI_WIDE_GAMUT4,
                T::Linear,
            )
            .with_aliases(&["lin_awg4"]),
            ColorSpace::new(
                "Linear ARRI Wide Gamut 3",
                Primaries::ARRI_WIDE_GAMUT3,
                T::Linear,
            )
            .with_aliases(&["lin_awg3"]),
            ColorSpace::new(
                "Sony S-Log3 S-Gamut3.Cine",
                Primaries::S_GAMUT3_CINE,
                T::SLog3,
            )
            .with_aliases(&[
                "slog3_sgamut3cine",
                "S-Log3 S-Gamut3.Cine",
                "Input - Sony - S-Log3 - S-Gamut3.Cine",
            ]),
            ColorSpace::new("Sony S-Log3 S-Gamut3", Primaries::S_GAMUT3, T::SLog3)
                .with_aliases(&["slog3_sgamut3", "S-Log3 S-Gamut3"]),
            ColorSpace::new("Linear S-Gamut3", Primaries::S_GAMUT3, T::Linear)
                .with_aliases(&["lin_sgamut3"]),
            ColorSpace::new("Linear S-Gamut3.Cine", Primaries::S_GAMUT3_CINE, T::Linear)
                .with_aliases(&["lin_sgamut3cine"]),
            ColorSpace::new("RED Log3G10 RWG", Primaries::RED_WIDE_GAMUT, T::Log3G10).with_aliases(
                &[
                    "log3g10_rwg",
                    "RED Log3G10 REDWideGamutRGB",
                    "Input - RED - Log3G10 - REDWideGamutRGB",
                ],
            ),
            ColorSpace::new(
                "Linear REDWideGamutRGB",
                Primaries::RED_WIDE_GAMUT,
                T::Linear,
            )
            .with_aliases(&["lin_rwg"]),
            ColorSpace::new("Panasonic V-Log V-Gamut", Primaries::V_GAMUT, T::VLog).with_aliases(
                &[
                    "vlog_vgamut",
                    "V-Log",
                    "Input - Panasonic - V-Log - V-Gamut",
                ],
            ),
            ColorSpace::new("Linear V-Gamut", Primaries::V_GAMUT, T::Linear)
                .with_aliases(&["lin_vgamut"]),
            ColorSpace::new(
                "Canon Log2 Cinema Gamut",
                Primaries::CANON_CINEMA_GAMUT,
                T::CanonLog2,
            )
            .with_aliases(&["canonlog2_cgamut", "Canon Log 2"]),
            ColorSpace::new(
                "Canon Log3 Cinema Gamut",
                Primaries::CANON_CINEMA_GAMUT,
                T::CanonLog3,
            )
            .with_aliases(&["canonlog3_cgamut", "Canon Log 3"]),
            ColorSpace::new(
                "Linear Canon Cinema Gamut",
                Primaries::CANON_CINEMA_GAMUT,
                T::Linear,
            )
            .with_aliases(&["lin_canoncgamut"]),
            ColorSpace::new("Gamma 2.2 Rec.709", Primaries::REC709, T::Gamma(2.2))
                .with_aliases(&["g22_rec709"])
                .display_referred(),
        ]
    }

    /// Look a built-in colorspace up by canonical name or alias.
    pub fn builtin(name: &str) -> Option<ColorSpace> {
        Self::builtins().into_iter().find(|c| c.matches(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_to_xyz_matches_reference() {
        let m = Primaries::REC709.rgb_to_xyz();
        let want = Mat3([
            [0.4124, 0.3576, 0.1805],
            [0.2126, 0.7152, 0.0722],
            [0.0193, 0.1192, 0.9505],
        ]);
        assert!(m.max_abs_diff(&want) < 1e-3, "{m:?}");
    }

    #[test]
    fn ap1_to_xyz_matches_reference() {
        let m = Primaries::ACES_AP1.rgb_to_xyz();
        let want = Mat3([
            [0.6624541811, 0.1340042065, 0.1561876870],
            [0.2722287168, 0.6740817658, 0.0536895174],
            [-0.0055746495, 0.0040607335, 1.0103391003],
        ]);
        assert!(m.max_abs_diff(&want) < 1e-3, "{m:?}");
    }

    #[test]
    fn ap0_to_xyz_matches_reference() {
        let m = Primaries::ACES_AP0.rgb_to_xyz();
        let want = Mat3([
            [0.9525523959, 0.0, 0.0000936786],
            [0.3439664498, 0.7281660966, -0.0721325464],
            [0.0, 0.0, 1.0088251844],
        ]);
        assert!(m.max_abs_diff(&want) < 1e-3, "{m:?}");
    }

    #[test]
    fn rec2020_to_xyz_matches_reference() {
        let m = Primaries::REC2020.rgb_to_xyz();
        let want = Mat3([
            [0.6370, 0.1446, 0.1689],
            [0.2627, 0.6780, 0.0593],
            [0.0, 0.0281, 1.0610],
        ]);
        assert!(m.max_abs_diff(&want) < 1e-3, "{m:?}");
    }

    #[test]
    fn bradford_d65_to_d50_matches_reference() {
        let m = bradford_adaptation(WHITE_D65, WHITE_D50);
        let want = Mat3([
            [1.0478112, 0.0228866, -0.0501270],
            [0.0295424, 0.9904844, -0.0170491],
            [-0.0092345, 0.0150436, 0.7521316],
        ]);
        assert!(m.max_abs_diff(&want) < 2e-3, "{m:?}");
    }

    #[test]
    fn white_is_preserved_between_spaces() {
        let m = rgb_to_rgb_matrix(&Primaries::ARRI_WIDE_GAMUT4, &Primaries::ACES_AP1);
        let w = m.apply([1.0, 1.0, 1.0]);
        for c in w {
            assert!((c - 1.0).abs() < 1e-6, "{w:?}");
        }
    }

    #[test]
    fn inverse_round_trip() {
        let m = Primaries::RED_WIDE_GAMUT.rgb_to_xyz();
        let i = m.inverse().unwrap();
        assert!(m.mul(&i).is_identity(1e-9));
    }

    #[test]
    fn builtin_lookup_by_alias() {
        assert_eq!(ColorSpace::builtin("lin_ap1").unwrap().name, "ACEScg");
        assert!(ColorSpace::builtin("nope").is_none());
    }

    #[test]
    fn daylight_locus_d65() {
        let xy = daylight_chromaticity(6504.0);
        assert!(
            (xy[0] - 0.3127).abs() < 2e-3 && (xy[1] - 0.3290).abs() < 2e-3,
            "{xy:?}"
        );
    }

    #[test]
    fn planckian_locus_and_cct() {
        // Published black-body chromaticities.
        let p = planckian_chromaticity(6500.0);
        assert!(
            (p[0] - 0.3135).abs() < 1e-3 && (p[1] - 0.3237).abs() < 1e-3,
            "{p:?}"
        );
        let p = planckian_chromaticity(3200.0);
        assert!(
            (p[0] - 0.4234).abs() < 1e-3 && (p[1] - 0.3990).abs() < 1e-3,
            "{p:?}"
        );
        // D65 sits ≈ 0.003 above the locus (green side).
        let d65 = xy_to_uv(WHITE_D65);
        let l = xy_to_uv(planckian_chromaticity(6504.0));
        let n = planckian_normal_uv(6504.0);
        let duv = (d65[0] - l[0]) * n[0] + (d65[1] - l[1]) * n[1];
        assert!(duv > 0.002 && duv < 0.004, "{duv}");
        assert!((cct_mccamy(WHITE_D65) - 6504.0).abs() < 10.0);
        assert!(
            (cct_mccamy(WHITE_D50) - 5003.0).abs() < 15.0,
            "{}",
            cct_mccamy(WHITE_D50)
        );
        // uv round trip and a Duv shift lands where asked.
        let back = uv_to_xy(xy_to_uv(WHITE_ACES));
        assert!((back[0] - WHITE_ACES[0]).abs() < 1e-12 && (back[1] - WHITE_ACES[1]).abs() < 1e-12);
        let shifted = xy_to_uv(planckian_with_duv(5000.0, 0.01));
        let base = xy_to_uv(planckian_chromaticity(5000.0));
        let d = ((shifted[0] - base[0]).powi(2) + (shifted[1] - base[1]).powi(2)).sqrt();
        assert!((d - 0.01).abs() < 1e-9);
        assert!(shifted[1] > base[1], "positive Duv is the green side");
    }
}
