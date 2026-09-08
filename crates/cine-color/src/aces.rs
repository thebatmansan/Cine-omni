//! ACES: AP0/AP1 matrices, ACES2065-1 / ACEScg / ACEScc / ACEScct
//! conversions, input transforms for camera encodings and output transforms.
//!
//! Two output paths are provided:
//!
//! * [`rrt_odt_fit_srgb`] — Stephen Hill's analytic fit of the ACES 1.x
//!   RRT + sRGB ODT (`ACESInputMat` / `RRTAndODTFit` / `ACESOutputMat`). This
//!   is an **approximation** of the official CTL reference, accurate to a few
//!   percent for SDR sRGB output; it is what most real-time renderers use.
//! * [`OutputTransform`] — a structured, parametric display transform: a
//!   Daniele-style sigmoid tonescale (the curve family used by ACES 2.0) in
//!   scene-linear AP1, applied either per channel or on max(RGB) with
//!   preserved ratios (hue preserving), followed by gamut conversion and the
//!   display encoding. It is not the official CTL either, but it is exact,
//!   documented, and parametric in peak luminance.

#![allow(clippy::needless_range_loop)]

use crate::processor::{Op, Processor};
use crate::space::{
    bradford_adaptation, rgb_to_rgb_matrix, Mat3, Primaries, WHITE_ACES, WHITE_D65,
};
use crate::transfer::{hlg_eotf_nits, hlg_inverse_eotf_nits, Transfer};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Matrices
// ---------------------------------------------------------------------------

/// AP0 → AP1 (ACES2065-1 → ACEScg), computed from the primaries.
pub fn ap0_to_ap1() -> Mat3 {
    rgb_to_rgb_matrix(&Primaries::ACES_AP0, &Primaries::ACES_AP1)
}

/// AP1 → AP0 (ACEScg → ACES2065-1).
pub fn ap1_to_ap0() -> Mat3 {
    rgb_to_rgb_matrix(&Primaries::ACES_AP1, &Primaries::ACES_AP0)
}

/// Published AP0 → AP1 matrix (ACES 1.x reference values).
pub const AP0_TO_AP1_REF: Mat3 = Mat3([
    [1.4514393161, -0.2365107469, -0.2149285693],
    [-0.0765537734, 1.1762296998, -0.0996759264],
    [0.0083161484, -0.0060324498, 0.9977163014],
]);

/// Published AP1 → AP0 matrix.
pub const AP1_TO_AP0_REF: Mat3 = Mat3([
    [0.6954522414, 0.1406786965, 0.1638690622],
    [0.0447945634, 0.8596711185, 0.0955343182],
    [-0.0055258826, 0.0040252103, 1.0015006723],
]);

/// AP1 luminance weights (Y row of AP1→XYZ).
pub const AP1_LUMA: [f64; 3] = [0.2722287168, 0.6740817658, 0.0536895174];

// ---------------------------------------------------------------------------
// Encodings
// ---------------------------------------------------------------------------

/// ACES2065-1 (AP0 linear) → ACEScg (AP1 linear).
pub fn aces2065_to_acescg(rgb: [f64; 3]) -> [f64; 3] {
    ap0_to_ap1().apply(rgb)
}

/// ACEScg → ACES2065-1.
pub fn acescg_to_aces2065(rgb: [f64; 3]) -> [f64; 3] {
    ap1_to_ap0().apply(rgb)
}

/// ACEScg → ACEScct.
pub fn acescg_to_acescct(rgb: [f64; 3]) -> [f64; 3] {
    [
        Transfer::ACEScct.encode(rgb[0]),
        Transfer::ACEScct.encode(rgb[1]),
        Transfer::ACEScct.encode(rgb[2]),
    ]
}

/// ACEScct → ACEScg.
pub fn acescct_to_acescg(rgb: [f64; 3]) -> [f64; 3] {
    [
        Transfer::ACEScct.decode(rgb[0]),
        Transfer::ACEScct.decode(rgb[1]),
        Transfer::ACEScct.decode(rgb[2]),
    ]
}

/// ACEScg → ACEScc.
pub fn acescg_to_acescc(rgb: [f64; 3]) -> [f64; 3] {
    [
        Transfer::ACEScc.encode(rgb[0]),
        Transfer::ACEScc.encode(rgb[1]),
        Transfer::ACEScc.encode(rgb[2]),
    ]
}

/// ACEScc → ACEScg.
pub fn acescc_to_acescg(rgb: [f64; 3]) -> [f64; 3] {
    [
        Transfer::ACEScc.decode(rgb[0]),
        Transfer::ACEScc.decode(rgb[1]),
        Transfer::ACEScc.decode(rgb[2]),
    ]
}

/// ACES2065-1 → ACEScct.
pub fn aces2065_to_acescct(rgb: [f64; 3]) -> [f64; 3] {
    acescg_to_acescct(aces2065_to_acescg(rgb))
}

/// ACEScct → ACES2065-1.
pub fn acescct_to_aces2065(rgb: [f64; 3]) -> [f64; 3] {
    acescg_to_aces2065(acescct_to_acescg(rgb))
}

// ---------------------------------------------------------------------------
// Hill fit
// ---------------------------------------------------------------------------

/// Hill's `ACESInputMat`: linear Rec.709 → RRT saturation space.
pub const ACES_INPUT_MAT: Mat3 = Mat3([
    [0.59719, 0.35458, 0.04823],
    [0.07600, 0.90834, 0.01566],
    [0.02840, 0.13383, 0.83777],
]);

/// Hill's `ACESOutputMat`: ODT saturation space → linear Rec.709.
pub const ACES_OUTPUT_MAT: Mat3 = Mat3([
    [1.60475, -0.53108, -0.07367],
    [-0.10208, 1.10813, -0.00605],
    [-0.00327, -0.07276, 1.07602],
]);

/// Hill's rational polynomial fit of RRT + ODT (per channel).
pub fn rrt_and_odt_fit(v: f64) -> f64 {
    let a = v * (v + 0.0245786) - 0.000090537;
    let b = v * (0.983729 * v + 0.4329510) + 0.238081;
    a / b
}

/// The fitted RRT+ODT on **linear Rec.709** input, returning linear Rec.709
/// display values in [0, 1] (no encoding).
pub fn aces_fitted_linear_rec709(rgb: [f64; 3]) -> [f64; 3] {
    let v = ACES_INPUT_MAT.apply(rgb);
    let t = [
        rrt_and_odt_fit(v[0]),
        rrt_and_odt_fit(v[1]),
        rrt_and_odt_fit(v[2]),
    ];
    let o = ACES_OUTPUT_MAT.apply(t);
    [
        o[0].clamp(0.0, 1.0),
        o[1].clamp(0.0, 1.0),
        o[2].clamp(0.0, 1.0),
    ]
}

/// ACEScg → sRGB encoded display values through the fitted RRT + sRGB ODT.
///
/// The fit's input space is linear Rec.709, so ACEScg is first converted
/// (AP1/D60 → Rec.709/D65, Bradford). Mid gray 0.18 lands at ≈0.10 display
/// linear (≈0.36 sRGB encoded), matching the official ACES 1.x sRGB ODT.
pub fn rrt_odt_fit_srgb(rgb_acescg: [f32; 3]) -> [f32; 3] {
    let m = rgb_to_rgb_matrix(&Primaries::ACES_AP1, &Primaries::REC709);
    let lin709 = m.apply([
        rgb_acescg[0] as f64,
        rgb_acescg[1] as f64,
        rgb_acescg[2] as f64,
    ]);
    let o = aces_fitted_linear_rec709(lin709);
    [
        Transfer::SRgb.encode(o[0]) as f32,
        Transfer::SRgb.encode(o[1]) as f32,
        Transfer::SRgb.encode(o[2]) as f32,
    ]
}

// ---------------------------------------------------------------------------
// Structured output transform
// ---------------------------------------------------------------------------

/// Target display of an [`OutputTransform`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Display {
    /// Rec.709 primaries, BT.1886 (gamma 2.4), 100 nits.
    Rec709,
    /// Rec.709 primaries, sRGB piecewise curve, 100 nits.
    SRgb,
    /// P3 primaries, D65 white, sRGB curve (Display P3), 100 nits.
    P3D65,
    /// Rec.2020 primaries, ST.2084 PQ, 1000 nits peak.
    Rec2020Pq1000,
    /// Rec.2020 primaries, HLG (BT.2100, 1000-nit reference).
    Rec2020Hlg,
}

impl Display {
    /// Display primaries.
    pub fn primaries(&self) -> Primaries {
        match self {
            Display::Rec709 | Display::SRgb => Primaries::REC709,
            Display::P3D65 => Primaries::DISPLAY_P3,
            Display::Rec2020Pq1000 | Display::Rec2020Hlg => Primaries::REC2020,
        }
    }

    /// Nominal peak luminance in nits.
    pub fn peak_nits(&self) -> f64 {
        match self {
            Display::Rec709 | Display::SRgb | Display::P3D65 => 100.0,
            Display::Rec2020Pq1000 | Display::Rec2020Hlg => 1000.0,
        }
    }

    /// Encode display light in nits to code values.
    pub fn encode_nits(&self, nits: [f64; 3]) -> [f64; 3] {
        match self {
            Display::Rec709 => {
                nits.map(|n| Transfer::Gamma(2.4).encode((n / 100.0).clamp(0.0, 1.0)))
            }
            Display::SRgb | Display::P3D65 => {
                nits.map(|n| Transfer::SRgb.encode((n / 100.0).clamp(0.0, 1.0)))
            }
            Display::Rec2020Pq1000 => {
                nits.map(|n| Transfer::Pq.encode((n / 10_000.0).clamp(0.0, 1.0)))
            }
            Display::Rec2020Hlg => {
                let c = nits.map(|n| n.clamp(0.0, 1000.0));
                hlg_inverse_eotf_nits(c, 1000.0)
            }
        }
    }

    /// Decode display code values to display light in nits (inverse of
    /// [`Display::encode_nits`]; code values are clamped to [0, 1]).
    pub fn decode_nits(&self, code: [f64; 3]) -> [f64; 3] {
        match self {
            Display::Rec709 => code.map(|c| Transfer::Gamma(2.4).decode(c.clamp(0.0, 1.0)) * 100.0),
            Display::SRgb | Display::P3D65 => {
                code.map(|c| Transfer::SRgb.decode(c.clamp(0.0, 1.0)) * 100.0)
            }
            Display::Rec2020Pq1000 => {
                code.map(|c| Transfer::Pq.decode(c.clamp(0.0, 1.0)) * 10_000.0)
            }
            Display::Rec2020Hlg => hlg_eotf_nits(code.map(|c| c.clamp(0.0, 1.0)), 1000.0),
        }
    }

    /// OCIO-style display name.
    pub fn name(&self) -> &'static str {
        match self {
            Display::Rec709 => "Rec.709",
            Display::SRgb => "sRGB",
            Display::P3D65 => "Display P3",
            Display::Rec2020Pq1000 => "Rec.2100-PQ",
            Display::Rec2020Hlg => "Rec.2100-HLG",
        }
    }
}

/// Parameters of the sigmoid tonescale (Daniele Siragusano's "Evo" curve).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TonescaleParams {
    /// Display peak luminance in nits.
    pub peak_nits: f64,
    /// Normalizing luminance (100 nits = 1.0).
    pub n_r: f64,
    /// Contrast / surround exponent.
    pub contrast: f64,
    /// Scene-referred mid gray (0.18).
    pub gray_in: f64,
    /// Display luminance of mid gray on a 100-nit display, in nits.
    pub gray_out_nits: f64,
    /// How much mid gray rises with peak luminance.
    pub gray_change: f64,
    /// Toe / flare amount.
    pub toe: f64,
    /// Scene value hitting the roof at 100 nits.
    pub r_hit_min: f64,
    /// Scene value hitting the roof at 10 000 nits.
    pub r_hit_max: f64,
}

impl TonescaleParams {
    /// Reference parameters for a display of `peak_nits`.
    pub fn for_peak(peak_nits: f64) -> Self {
        TonescaleParams {
            peak_nits,
            n_r: 100.0,
            contrast: 1.15,
            gray_in: 0.18,
            gray_out_nits: 10.013,
            gray_change: 0.14,
            toe: 0.04,
            r_hit_min: 128.0,
            r_hit_max: 896.0,
        }
    }

    /// Derived curve constants `(m_2, s_2)`.
    fn constants(&self) -> (f64, f64) {
        let n = self.peak_nits;
        let n_r = self.n_r;
        let g = self.contrast;
        let t_1 = self.toe;
        let r_hit = self.r_hit_min
            + (self.r_hit_max - self.r_hit_min) * ((n / n_r).ln() / (10_000.0f64 / 100.0).ln());
        let m_0 = n / n_r;
        let m_1 = 0.5 * (m_0 + (m_0 * (m_0 + 4.0 * t_1)).sqrt());
        let u = ((r_hit / m_1) / ((r_hit / m_1) + 1.0)).powf(g);
        let m = m_1 / u;
        let w_i = (n / 100.0).ln() / 2f64.ln();
        let c_t = self.gray_out_nits / n_r * (1.0 + w_i * self.gray_change);
        let g_ip = 0.5 * (c_t + (c_t * (c_t + 4.0 * t_1)).sqrt());
        let p = (g_ip / m).powf(1.0 / g);
        let g_ipp2 = -m_1 * p / (p - 1.0);
        let w_2 = self.gray_in / g_ipp2;
        let s_2 = w_2 * m_1;
        let u_2 = ((r_hit / m_1) / ((r_hit / m_1) + w_2)).powf(g);
        let m_2 = m_1 / u_2;
        (m_2, s_2)
    }

    /// Scene-linear value → display luminance in nits (clamped to the peak).
    pub fn apply_nits(&self, x: f64) -> f64 {
        let (m_2, s_2) = self.constants();
        let x = x.max(0.0);
        let f = m_2 * (x / (x + s_2)).powf(self.contrast);
        let h = (f * f / (f + self.toe)).max(0.0);
        (h * self.n_r).min(self.peak_nits)
    }

    /// Display luminance in nits → scene-linear value: the analytic inverse
    /// of [`TonescaleParams::apply_nits`].
    ///
    /// With `h = nits / n_r`, the toe `h = f² / (f + toe)` is a quadratic in
    /// `f` (positive root `f = (h + √(h² + 4·h·toe)) / 2`) and the sigmoid
    /// `f = m₂ (x / (x + s₂))^c` inverts to `x = s₂ r / (1 − r)` with
    /// `r = (f / m₂)^(1/c)`. The round trip is exact to floating-point
    /// precision (≈1e-12 relative) below the peak; input above the peak is
    /// clamped to it and maps to the smallest scene value that reaches it.
    pub fn inverse_nits(&self, nits: f64) -> f64 {
        let (m_2, s_2) = self.constants();
        let h = nits.clamp(0.0, self.peak_nits) / self.n_r;
        if h <= 0.0 {
            return 0.0;
        }
        let f = 0.5 * (h + (h * h + 4.0 * h * self.toe).sqrt());
        let r = (f / m_2).max(0.0).powf(1.0 / self.contrast);
        if r >= 1.0 {
            return 65504.0;
        }
        s_2 * r / (1.0 - r)
    }
}

/// Whether the tonescale is applied per channel or on the RGB maximum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HueMode {
    /// Independent per-channel tone mapping (classic ACES 1.x look: hue skews
    /// toward the primaries in highlights, "filmic" desaturation).
    #[default]
    PerChannel,
    /// Tonescale max(R,G,B) and scale the pixel by the ratio — preserves hue
    /// and saturation (highlights do not desaturate; clipped by the display).
    MaxRgb,
}

/// A parametric ACES-style display transform (AP1 scene-linear → display code values).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputTransform {
    pub display: Display,
    pub tonescale: TonescaleParams,
    pub hue_mode: HueMode,
}

impl OutputTransform {
    /// Default transform for a display (reference peak, per-channel).
    pub fn for_display(display: Display) -> Self {
        OutputTransform {
            display,
            tonescale: TonescaleParams::for_peak(display.peak_nits()),
            hue_mode: HueMode::PerChannel,
        }
    }

    /// Select the hue mode (builder style).
    pub fn with_hue_mode(mut self, mode: HueMode) -> Self {
        self.hue_mode = mode;
        self
    }

    /// AP1 scene-linear → display light in nits (display primaries).
    pub fn to_display_nits(&self, acescg: [f64; 3]) -> [f64; 3] {
        let toned = match self.hue_mode {
            HueMode::PerChannel => acescg.map(|c| self.tonescale.apply_nits(c)),
            HueMode::MaxRgb => {
                let m = acescg[0].max(acescg[1]).max(acescg[2]);
                if m <= 0.0 {
                    [0.0, 0.0, 0.0]
                } else {
                    let t = self.tonescale.apply_nits(m);
                    let k = t / m;
                    [
                        acescg[0].max(0.0) * k,
                        acescg[1].max(0.0) * k,
                        acescg[2].max(0.0) * k,
                    ]
                }
            }
        };
        let m = rgb_to_rgb_matrix(&Primaries::ACES_AP1, &self.display.primaries());
        let out = m.apply(toned);
        let peak = self.tonescale.peak_nits;
        out.map(|c| c.clamp(0.0, peak))
    }

    /// Inverse of the tonescale stage: tone-mapped AP1 (in nits) → AP1 scene-linear.
    fn untone_ap1(&self, toned: [f64; 3]) -> [f64; 3] {
        match self.hue_mode {
            HueMode::PerChannel => toned.map(|c| self.tonescale.inverse_nits(c)),
            HueMode::MaxRgb => {
                // forward: toned = max(rgb, 0) · T(m) / m with m = max(rgb), so max(toned) = T(m)
                let t = toned[0].max(toned[1]).max(toned[2]);
                let m = self.tonescale.inverse_nits(t);
                if m <= 0.0 || t <= 0.0 {
                    return [0.0, 0.0, 0.0];
                }
                let k = self.tonescale.apply_nits(m) / m;
                toned.map(|c| c.max(0.0) / k)
            }
        }
    }

    /// Display light in nits (display primaries) → AP1 scene-linear: the
    /// inverse of [`OutputTransform::to_display_nits`]. Exact (to floating
    /// point) for values inside the display gamut and below the peak; the
    /// forward clamp makes out-of-gamut / clipped values unrecoverable.
    pub fn from_display_nits(&self, nits: [f64; 3]) -> [f64; 3] {
        let m = rgb_to_rgb_matrix(&self.display.primaries(), &Primaries::ACES_AP1);
        self.untone_ap1(m.apply(nits))
    }

    /// AP1 scene-linear → encoded display code values.
    pub fn apply(&self, acescg: [f32; 3]) -> [f32; 3] {
        let nits = self.to_display_nits([acescg[0] as f64, acescg[1] as f64, acescg[2] as f64]);
        let e = self.display.encode_nits(nits);
        [e[0] as f32, e[1] as f32, e[2] as f32]
    }

    /// Encoded display code values → AP1 scene-linear (inverse of [`OutputTransform::apply`]).
    pub fn apply_inverse(&self, code: [f32; 3]) -> [f32; 3] {
        let nits = self
            .display
            .decode_nits([code[0] as f64, code[1] as f64, code[2] as f64]);
        let o = self.from_display_nits(nits);
        [o[0] as f32, o[1] as f32, o[2] as f32]
    }

    /// As an invertible processor op (AP1 linear in, display code values out).
    pub fn processor(&self) -> Processor {
        let fwd = self.clone();
        let inv = self.clone();
        Processor::new(vec![Op::custom_invertible(
            &format!("ACES output {}", self.display.name()),
            move |c| fwd.display.encode_nits(fwd.to_display_nits(c)),
            move |c| inv.from_display_nits(inv.display.decode_nits(c)),
        )])
    }

    /// AP1 linear → CIE XYZ (D65) in units of `n_r` (1.0 = 100 nits), the OCIO
    /// display-reference convention. Used for view transforms in configs.
    /// The op is invertible (see [`OutputTransform::from_display_nits`]).
    pub fn to_xyz_d65_op(&self) -> Op {
        let fwd = self.clone();
        let inv = self.clone();
        let m = bradford_adaptation(WHITE_ACES, WHITE_D65).mul(&Primaries::ACES_AP1.rgb_to_xyz());
        let m_inv = m.inverse().unwrap_or(Mat3::IDENTITY);
        let disp_to_ap1 = rgb_to_rgb_matrix(&self.display.primaries(), &Primaries::ACES_AP1);
        Op::custom_invertible(
            &format!("ACES tonescale {}", self.display.name()),
            move |c| {
                let nits = fwd.to_display_nits(c);
                // back to AP1 (already clamped to display gamut) then to XYZ
                let ap1 = disp_to_ap1.apply(nits);
                let xyz = m.apply(ap1);
                xyz.map(|v| v / fwd.tonescale.n_r)
            },
            move |xyz| {
                let toned = m_inv.apply(xyz.map(|v| v * inv.tonescale.n_r));
                inv.untone_ap1(toned)
            },
        )
    }
}

// ---------------------------------------------------------------------------
// Input transforms
// ---------------------------------------------------------------------------

/// Camera encodings with a built-in IDT.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Camera {
    /// ARRI LogC4 / ARRI Wide Gamut 4.
    LogC4,
    /// ARRI LogC3 (EI 800) / ARRI Wide Gamut 3.
    LogC3,
    /// Sony S-Log3 / S-Gamut3.Cine.
    SLog3SGamut3Cine,
    /// Sony S-Log3 / S-Gamut3.
    SLog3SGamut3,
    /// RED Log3G10 / REDWideGamutRGB.
    Log3G10Rwg,
    /// Panasonic V-Log / V-Gamut.
    VLog,
    /// Canon Log 2 / Cinema Gamut.
    CanonLog2,
    /// Canon Log 3 / Cinema Gamut.
    CanonLog3,
}

impl Camera {
    /// Transfer function and native gamut.
    pub fn encoding(&self) -> (Transfer, Primaries) {
        match self {
            Camera::LogC4 => (Transfer::LogC4, Primaries::ARRI_WIDE_GAMUT4),
            Camera::LogC3 => (Transfer::LogC3, Primaries::ARRI_WIDE_GAMUT3),
            Camera::SLog3SGamut3Cine => (Transfer::SLog3, Primaries::S_GAMUT3_CINE),
            Camera::SLog3SGamut3 => (Transfer::SLog3, Primaries::S_GAMUT3),
            Camera::Log3G10Rwg => (Transfer::Log3G10, Primaries::RED_WIDE_GAMUT),
            Camera::VLog => (Transfer::VLog, Primaries::V_GAMUT),
            Camera::CanonLog2 => (Transfer::CanonLog2, Primaries::CANON_CINEMA_GAMUT),
            Camera::CanonLog3 => (Transfer::CanonLog3, Primaries::CANON_CINEMA_GAMUT),
        }
    }

    /// Built-in colorspace name.
    pub fn colorspace_name(&self) -> &'static str {
        match self {
            Camera::LogC4 => "ARRI LogC4",
            Camera::LogC3 => "ARRI LogC3",
            Camera::SLog3SGamut3Cine => "Sony S-Log3 S-Gamut3.Cine",
            Camera::SLog3SGamut3 => "Sony S-Log3 S-Gamut3",
            Camera::Log3G10Rwg => "RED Log3G10 RWG",
            Camera::VLog => "Panasonic V-Log V-Gamut",
            Camera::CanonLog2 => "Canon Log2 Cinema Gamut",
            Camera::CanonLog3 => "Canon Log3 Cinema Gamut",
        }
    }
}

/// Input device transform: camera log encoding → ACEScg (AP1 linear).
pub fn idt(camera: Camera) -> Processor {
    let (t, p) = camera.encoding();
    Processor::new(vec![
        Op::decode(t),
        Op::matrix(rgb_to_rgb_matrix(&p, &Primaries::ACES_AP1)),
    ])
}

/// Input device transform to ACES2065-1 (AP0 linear).
pub fn idt_ap0(camera: Camera) -> Processor {
    let (t, p) = camera.encoding();
    Processor::new(vec![
        Op::decode(t),
        Op::matrix(rgb_to_rgb_matrix(&p, &Primaries::ACES_AP0)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ap_matrices_match_reference() {
        assert!(
            ap0_to_ap1().max_abs_diff(&AP0_TO_AP1_REF) < 1e-4,
            "{:?}",
            ap0_to_ap1()
        );
        assert!(
            ap1_to_ap0().max_abs_diff(&AP1_TO_AP0_REF) < 1e-4,
            "{:?}",
            ap1_to_ap0()
        );
    }

    #[test]
    fn acescct_reference_values() {
        // ACEScct of 0.18 = (log2(0.18) + 9.72) / 17.52
        const CCT_GRAY: f64 = 0.4135884;
        let cct = acescg_to_acescct([0.18, 0.18, 0.18]);
        assert!((cct[0] - CCT_GRAY).abs() < 1e-6, "{cct:?}");
        let lin = acescct_to_acescg([CCT_GRAY; 3]);
        assert!((lin[0] - 0.18).abs() < 1e-6, "{lin:?}");
        // toe: linear below the break, continuous at it
        let brk = 0.0078125;
        assert!((Transfer::ACEScct.encode(brk) - 0.155251141552511).abs() < 1e-9);
        assert!(
            (Transfer::ACEScct.encode(brk + 1e-9) - Transfer::ACEScct.encode(brk - 1e-9)).abs()
                < 1e-7
        );
        let cg = aces2065_to_acescg([0.18, 0.18, 0.18]);
        for c in cg {
            assert!((c - 0.18).abs() < 1e-6);
        }
        let back = acescct_to_aces2065(aces2065_to_acescct([0.1, 0.5, 0.9]));
        assert!((back[0] - 0.1).abs() < 1e-6 && (back[2] - 0.9).abs() < 1e-6);
    }

    #[test]
    fn fit_mid_gray_sanity() {
        let o = rrt_odt_fit_srgb([0.18, 0.18, 0.18]);
        // Official ACES sRGB ODT puts 18 % gray at ≈10 % display linear ≈ 0.36 sRGB.
        assert!(o[0] > 0.30 && o[0] < 0.50, "{o:?}");
        assert!(
            (o[0] - o[1]).abs() < 2e-3 && (o[1] - o[2]).abs() < 2e-3,
            "{o:?}"
        );
        let black = rrt_odt_fit_srgb([0.0; 3]);
        assert!(black[0] < 0.02);
        let white = rrt_odt_fit_srgb([16.0; 3]);
        assert!(white[0] > 0.97);
        // monotone
        let mut last = -1.0;
        for i in 0..200 {
            let x = i as f32 * 0.05;
            let v = rrt_odt_fit_srgb([x; 3])[1];
            assert!(v >= last - 1e-6);
            last = v;
        }
    }

    #[test]
    fn tonescale_reference_points() {
        let ts = TonescaleParams::for_peak(100.0);
        assert!(
            (ts.apply_nits(0.18) - 10.0).abs() < 0.1,
            "{}",
            ts.apply_nits(0.18)
        );
        assert!(ts.apply_nits(0.0) == 0.0);
        assert!((ts.apply_nits(1000.0) - 100.0).abs() < 1e-6);
        let hdr = TonescaleParams::for_peak(1000.0);
        let g = hdr.apply_nits(0.18);
        assert!(g > 10.0 && g < 20.0, "{g}");
        assert!((hdr.apply_nits(10_000.0) - 1000.0).abs() < 1e-6);
    }

    #[test]
    fn tonescale_inverse_is_analytic() {
        for ts in [
            TonescaleParams::for_peak(100.0),
            TonescaleParams::for_peak(1000.0),
        ] {
            for x in [1e-4, 1e-3, 0.01, 0.1, 0.18, 0.5, 1.0, 4.0, 20.0, 100.0] {
                let n = ts.apply_nits(x);
                if n >= ts.peak_nits {
                    continue;
                }
                let back = ts.inverse_nits(n);
                assert!((back - x).abs() < 1e-9 * x.max(1.0), "{x} -> {n} -> {back}");
            }
            assert_eq!(ts.inverse_nits(0.0), 0.0);
            // peak maps to the scene value that first reaches it
            let top = ts.inverse_nits(ts.peak_nits);
            assert!((ts.apply_nits(top) - ts.peak_nits).abs() < 1e-6);
            assert!(ts.apply_nits(top * 0.999) < ts.peak_nits);
        }
    }

    #[test]
    fn output_transform_inverse_round_trips() {
        // The inverse is exact only where the forward transform did not clamp
        // (inside the display gamut and below the peak), so a colour whose
        // display value touches 0 or the peak is skipped.
        let colors = [
            [0.18, 0.18, 0.18],
            [0.3, 0.2, 0.1],
            [0.1, 0.25, 0.15],
            [0.05, 0.1, 0.3],
            [1.5, 1.2, 0.9],
        ];
        let mut checked = 0;
        for display in [
            Display::SRgb,
            Display::Rec709,
            Display::P3D65,
            Display::Rec2020Pq1000,
            Display::Rec2020Hlg,
        ] {
            for mode in [HueMode::PerChannel, HueMode::MaxRgb] {
                let ot = OutputTransform::for_display(display).with_hue_mode(mode);
                for c in colors {
                    let nits = ot.to_display_nits(c);
                    if nits
                        .iter()
                        .any(|n| *n <= 0.0 || *n >= ot.tonescale.peak_nits)
                    {
                        continue;
                    }
                    checked += 1;
                    let back = ot.from_display_nits(nits);
                    for i in 0..3 {
                        assert!(
                            (back[i] - c[i]).abs() < 1e-8,
                            "{display:?} {mode:?} {c:?} -> {nits:?} -> {back:?}"
                        );
                    }
                    let p = ot.processor();
                    let inv = p.inverse().expect("output transform is invertible");
                    let code = p.apply_rgb(&[c[0] as f32, c[1] as f32, c[2] as f32]);
                    let back = inv.apply_rgb(&code);
                    for i in 0..3 {
                        assert!(
                            (back[i] as f64 - c[i]).abs() < 1e-3,
                            "{display:?} {mode:?} {c:?} -> {code:?} -> {back:?}"
                        );
                    }
                }
                let op = ot.to_xyz_d65_op();
                let inv = op.inverse().expect("xyz op is invertible");
                let back = inv.apply(op.apply([0.18; 3]));
                for v in back {
                    assert!((v - 0.18).abs() < 1e-9, "{back:?}");
                }
            }
        }
        assert!(
            checked >= 40,
            "most test colours must be in gamut: {checked}"
        );
    }

    #[test]
    fn output_transform_gray_and_hue() {
        let ot = OutputTransform::for_display(Display::SRgb);
        let g = ot.apply([0.18; 3]);
        assert!(g[0] > 0.30 && g[0] < 0.50, "{g:?}");
        assert!((g[0] - g[1]).abs() < 1e-4);
        let hp = OutputTransform::for_display(Display::Rec709).with_hue_mode(HueMode::MaxRgb);
        let n = hp.to_display_nits([2.0, 0.5, 0.25]);
        // ratios preserved before gamut conversion: check AP1 values through the tonescale
        let m = hp.tonescale.apply_nits(2.0) / 2.0;
        let ap1 = rgb_to_rgb_matrix(&Primaries::REC709, &Primaries::ACES_AP1).apply(n);
        assert!((ap1[1] / ap1[0] - 0.25).abs() < 1e-3, "{ap1:?} {m}");
        let pq = OutputTransform::for_display(Display::Rec2020Pq1000).apply([0.18; 3]);
        assert!(pq[0] > 0.3 && pq[0] < 0.6, "{pq:?}");
        let hlg = OutputTransform::for_display(Display::Rec2020Hlg).apply([0.18; 3]);
        assert!(hlg[0] > 0.2 && hlg[0] < 0.6, "{hlg:?}");
    }

    #[test]
    fn idt_logc4_gray() {
        let code = Transfer::LogC4.encode(0.18) as f32;
        let p = idt(Camera::LogC4);
        let o = p.apply_rgb(&[code; 3]);
        for c in o {
            assert!((c - 0.18).abs() < 1e-3, "{o:?}");
        }
        for cam in [
            Camera::SLog3SGamut3Cine,
            Camera::Log3G10Rwg,
            Camera::VLog,
            Camera::LogC3,
            Camera::CanonLog2,
        ] {
            let (t, _) = cam.encoding();
            let code = t.encode(0.18) as f32;
            let o = idt(cam).apply_rgb(&[code; 3]);
            for c in o {
                assert!((c - 0.18).abs() < 1e-3, "{cam:?} {o:?}");
            }
        }
    }
}
