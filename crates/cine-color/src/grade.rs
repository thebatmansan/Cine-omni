//! Grading: primary wheels, ASC CDL, contrast, saturation, hue rotation,
//! temperature / tint, printer lights, curves, secondaries and the
//! serializable [`GradeStack`] applied with rayon.
//!
//! Every op works on linear or log RGB values as stored — the stack does not
//! convert spaces itself; `working_primaries` only informs the ops that need
//! chromaticities (temperature / tint).

#![allow(clippy::needless_range_loop)]

use crate::curve::Curves;
use crate::error::{Error, Result};
use crate::lut::Lut3D;
use crate::mask::{HslQualifier, Mask};
use crate::space::{
    bradford_adaptation, cct_mccamy, planckian_chromaticity, planckian_normal_uv, uv_to_xy,
    xy_to_uv, Mat3, PrimariesId, PLANCKIAN_MAX_K, PLANCKIAN_MIN_K,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use ucg_schema::Cdl;

/// Rec.709 luma weights used by saturation and CDL.
pub const LUMA_709: [f64; 3] = [0.2126, 0.7152, 0.0722];

// ---------------------------------------------------------------------------
// Primary wheels
// ---------------------------------------------------------------------------

/// Lift / gamma / gain / offset wheels with master (luminance) versions.
///
/// Per channel, with `L = lift + master_lift`, `G = gain * master_gain`,
/// `Y = gamma * master_gamma`, `O = offset + master_offset`:
///
/// ```text
/// out = G * (in + L * (1 - in)) ^ (1 / Y) + O
/// ```
///
/// so with everything else neutral `lift = 0.1` turns `0.5` into
/// `0.5 + 0.1 · (1 − 0.5) = 0.55`. Lift raises blacks while keeping white
/// fixed, gain scales the whole range, gamma bends mid-tones, offset shifts
/// everything. Negative bases are mirrored through the power so shadows below
/// zero do not produce NaN. All parameters are `f64` so the formula is exact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Wheels {
    pub lift: [f64; 3],
    pub gamma: [f64; 3],
    pub gain: [f64; 3],
    pub offset: [f64; 3],
    pub master_lift: f64,
    pub master_gamma: f64,
    pub master_gain: f64,
    pub master_offset: f64,
}

impl Default for Wheels {
    fn default() -> Self {
        Wheels {
            lift: [0.0; 3],
            gamma: [1.0; 3],
            gain: [1.0; 3],
            offset: [0.0; 3],
            master_lift: 0.0,
            master_gamma: 1.0,
            master_gain: 1.0,
            master_offset: 0.0,
        }
    }
}

fn sign_pow(x: f64, p: f64) -> f64 {
    if x < 0.0 {
        -((-x).powf(p))
    } else {
        x.powf(p)
    }
}

impl Wheels {
    /// True when all wheels are neutral.
    pub fn is_identity(&self) -> bool {
        self == &Wheels::default()
    }

    /// Effective per-channel `(lift, gamma, gain, offset)`.
    pub fn effective(&self) -> ([f64; 3], [f64; 3], [f64; 3], [f64; 3]) {
        let mut l = [0.0; 3];
        let mut y = [1.0; 3];
        let mut g = [1.0; 3];
        let mut o = [0.0; 3];
        for i in 0..3 {
            l[i] = self.lift[i] + self.master_lift;
            y[i] = (self.gamma[i] * self.master_gamma).max(1e-4);
            g[i] = self.gain[i] * self.master_gain;
            o[i] = self.offset[i] + self.master_offset;
        }
        (l, y, g, o)
    }

    /// Apply to one pixel.
    pub fn apply(&self, rgb: [f64; 3]) -> [f64; 3] {
        let (l, y, g, o) = self.effective();
        let mut out = [0.0; 3];
        for i in 0..3 {
            let base = rgb[i] + l[i] * (1.0 - rgb[i]);
            out[i] = g[i] * sign_pow(base, 1.0 / y[i]) + o[i];
        }
        out
    }
}

// ---------------------------------------------------------------------------
// CDL
// ---------------------------------------------------------------------------

/// ASC CDL forward: `out = clamp((in * slope + offset) ^ power)` per channel,
/// then saturation about Rec.709 luma. Negative bases are clamped to zero
/// before the power (ASC CDL v1.2 behaviour).
pub fn cdl_apply(rgb: [f64; 3], cdl: &Cdl) -> [f64; 3] {
    let mut o = [0.0; 3];
    for i in 0..3 {
        let v = (rgb[i] * cdl.slope[i] + cdl.offset[i]).max(0.0);
        o[i] = v.powf(cdl.power[i]);
    }
    let l = LUMA_709[0] * o[0] + LUMA_709[1] * o[1] + LUMA_709[2] * o[2];
    for v in o.iter_mut() {
        *v = l + (*v - l) * cdl.saturation;
    }
    o
}

/// Analytic inverse of [`cdl_apply`] (exact where the forward did not clamp).
pub fn cdl_apply_inverse(rgb: [f64; 3], cdl: &Cdl) -> [f64; 3] {
    let mut o = rgb;
    if cdl.saturation.abs() > 1e-12 {
        let l = LUMA_709[0] * o[0] + LUMA_709[1] * o[1] + LUMA_709[2] * o[2];
        for v in o.iter_mut() {
            *v = l + (*v - l) / cdl.saturation;
        }
    }
    for i in 0..3 {
        let p = if cdl.power[i].abs() < 1e-12 {
            1.0
        } else {
            1.0 / cdl.power[i]
        };
        let v = o[i].max(0.0).powf(p);
        let s = if cdl.slope[i].abs() < 1e-12 {
            1.0
        } else {
            cdl.slope[i]
        };
        o[i] = (v - cdl.offset[i]) / s;
    }
    o
}

/// True when a CDL is the identity.
pub fn cdl_is_identity(cdl: &Cdl) -> bool {
    let eps = 1e-9;
    cdl.slope.iter().all(|s| (s - 1.0).abs() < eps)
        && cdl.offset.iter().all(|o| o.abs() < eps)
        && cdl.power.iter().all(|p| (p - 1.0).abs() < eps)
        && (cdl.saturation - 1.0).abs() < eps
}

// ---------------------------------------------------------------------------
// Other primaries
// ---------------------------------------------------------------------------

/// Contrast about a pivot: `out = (in - pivot) * contrast + pivot`.
pub fn contrast_apply(rgb: [f64; 3], contrast: f64, pivot: f64) -> [f64; 3] {
    rgb.map(|v| (v - pivot) * contrast + pivot)
}

/// Saturation about Rec.709 luma.
pub fn saturation_apply(rgb: [f64; 3], sat: f64) -> [f64; 3] {
    let l = LUMA_709[0] * rgb[0] + LUMA_709[1] * rgb[1] + LUMA_709[2] * rgb[2];
    rgb.map(|v| l + (v - l) * sat)
}

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize3(v: [f64; 3]) -> [f64; 3] {
    let n = dot3(v, v).sqrt().max(1e-30);
    [v[0] / n, v[1] / n, v[2] / n]
}

/// Orthonormal basis `(e1, e2)` of the chroma plane `{ v : LUMA_709 · v = 0 }`
/// (`e1` points toward red, `e2` toward blue, Gram–Schmidt orthogonalized).
fn chroma_basis() -> ([f64; 3], [f64; 3]) {
    let w = LUMA_709;
    let red = [1.0 - w[0], -w[0], -w[0]];
    let blue = [-w[2], -w[2], 1.0 - w[2]];
    let e1 = normalize3(red);
    let d = dot3(blue, e1);
    let e2 = normalize3([
        blue[0] - d * e1[0],
        blue[1] - d * e1[1],
        blue[2] - d * e1[2],
    ]);
    (e1, e2)
}

/// Matrix of a hue rotation by `degrees` (it is linear).
///
/// A pixel is split as `rgb = Y·[1,1,1] + chroma` with `Y = LUMA_709 · rgb`,
/// which puts `chroma` in the plane orthogonal to the Rec.709 luma weights
/// (the YCbCr chroma plane). The chroma vector is rotated in that plane and
/// the pixel reassembled, so Rec.709 luma is preserved exactly:
/// `LUMA_709 · out = Y + LUMA_709 · chroma' = Y`.
pub fn hue_rotate_matrix(degrees: f64) -> Mat3 {
    let (e1, e2) = chroma_basis();
    let (s, c) = degrees.to_radians().sin_cos();
    // R = c (e1 e1ᵀ + e2 e2ᵀ) + s (e2 e1ᵀ − e1 e2ᵀ): rotation inside the chroma plane
    let mut rot = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            rot[i][j] = c * (e1[i] * e1[j] + e2[i] * e2[j]) + s * (e2[i] * e1[j] - e1[i] * e2[j]);
        }
    }
    // luma = [1,1,1] wᵀ ; chroma projector = I − luma
    let mut luma = [[0.0; 3]; 3];
    let mut proj = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            luma[i][j] = LUMA_709[j];
            proj[i][j] = if i == j { 1.0 } else { 0.0 } - LUMA_709[j];
        }
    }
    let rotated = Mat3(rot).mul(&Mat3(proj));
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = luma[i][j] + rotated.0[i][j];
        }
    }
    Mat3(out)
}

/// Rotate hue by `degrees` in the Rec.709 chroma plane (luma unchanged).
/// See [`hue_rotate_matrix`].
pub fn hue_rotate_apply(rgb: [f64; 3], degrees: f64) -> [f64; 3] {
    hue_rotate_matrix(degrees).apply(rgb)
}

/// Duv per tint point (`tint = ±100` spans ±0.05 Duv, a very strong cast).
pub const TINT_DUV_PER_POINT: f64 = 0.0005;

/// Temperature / tint as a Bradford chromatic adaptation in the working primaries.
///
/// The working white (its CCT estimated with McCamy's formula) is moved
/// along the Planckian locus by `−temperature_k` kelvin, then displaced
/// perpendicular to the locus by the tint (a Duv shift); the Bradford
/// adaptation from the working white to that target is applied in RGB
/// (`RGB → XYZ → CAT → RGB`), so a neutral becomes the target white.
///
/// * `temperature_k` is a **shift**: positive warms the image (neutrals move
///   toward a lower-CCT, redder white with `r > g > b`), negative cools it.
/// * `tint`: positive = magenta (below the locus), negative = green, in
///   units of [`TINT_DUV_PER_POINT`].
///
/// Zero temperature and tint give the identity matrix.
pub fn temperature_tint_matrix(temperature_k: f64, tint: f64, working: PrimariesId) -> Mat3 {
    if temperature_k.abs() < 1e-9 && tint.abs() < 1e-9 {
        return Mat3::IDENTITY;
    }
    let prim = working.primaries();
    let base_xy = prim.w;
    let base_cct = cct_mccamy(base_xy).clamp(PLANCKIAN_MIN_K, PLANCKIAN_MAX_K);
    let target_cct = (base_cct - temperature_k).clamp(PLANCKIAN_MIN_K, PLANCKIAN_MAX_K);
    // Keep the working white's own offset from the locus: shift by the locus
    // displacement between the two temperatures, then by the tint Duv.
    let base_uv = xy_to_uv(base_xy);
    let locus_base = xy_to_uv(planckian_chromaticity(base_cct));
    let locus_target = xy_to_uv(planckian_chromaticity(target_cct));
    let n = planckian_normal_uv(target_cct);
    let duv = -tint * TINT_DUV_PER_POINT;
    let target_uv = [
        base_uv[0] + (locus_target[0] - locus_base[0]) + duv * n[0],
        base_uv[1] + (locus_target[1] - locus_base[1]) + duv * n[1],
    ];
    let target_xy = uv_to_xy(target_uv);
    let cat = bradford_adaptation(base_xy, target_xy);
    prim.xyz_to_rgb().mul(&cat).mul(&prim.rgb_to_xyz())
}

/// Printer lights: one point = 0.025 log10 exposure (≈ 1/12 stop); positive
/// points brighten (Baselight / Resolve convention).
pub fn printer_lights_scale(points: [f64; 3]) -> [f64; 3] {
    points.map(|p| 10f64.powf(0.025 * p))
}

// ---------------------------------------------------------------------------
// GradeOp / GradeLayer / GradeStack
// ---------------------------------------------------------------------------

/// One grading operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum GradeOp {
    /// Exposure in stops (`out = in * 2^stops`).
    Exposure { stops: f64 },
    /// Lift / gamma / gain / offset wheels.
    Wheels(Wheels),
    /// ASC CDL.
    Cdl(Cdl),
    /// Contrast about a pivot (default pivot 0.18 for linear, 0.435 for log).
    Contrast { amount: f64, pivot: f64 },
    /// Saturation multiplier.
    Saturation { amount: f64 },
    /// Hue rotation in degrees (see [`hue_rotate_matrix`]).
    HueRotate { degrees: f64 },
    /// Temperature shift (K) and tint (see [`temperature_tint_matrix`]).
    TemperatureTint { temperature_k: f64, tint: f64 },
    /// Printer light points (R, G, B).
    PrinterLights { points: [f64; 3] },
    /// Curves.
    Curves(Curves),
    /// A baked 3D LUT.
    Lut3D(Lut3D),
    /// Constant matrix + offset.
    Matrix { m: Mat3, offset: [f64; 3] },
}

impl GradeOp {
    /// Apply to one pixel.
    pub fn apply(&self, rgb: [f64; 3], working: PrimariesId) -> [f64; 3] {
        match self {
            GradeOp::Exposure { stops } => {
                let k = 2f64.powf(*stops);
                rgb.map(|v| v * k)
            }
            GradeOp::Wheels(w) => w.apply(rgb),
            GradeOp::Cdl(c) => cdl_apply(rgb, c),
            GradeOp::Contrast { amount, pivot } => contrast_apply(rgb, *amount, *pivot),
            GradeOp::Saturation { amount } => saturation_apply(rgb, *amount),
            GradeOp::HueRotate { degrees } => hue_rotate_apply(rgb, *degrees),
            GradeOp::TemperatureTint {
                temperature_k,
                tint,
            } => temperature_tint_matrix(*temperature_k, *tint, working).apply(rgb),
            GradeOp::PrinterLights { points } => {
                let s = printer_lights_scale(*points);
                [rgb[0] * s[0], rgb[1] * s[1], rgb[2] * s[2]]
            }
            GradeOp::Curves(c) => {
                let o = c.apply([rgb[0] as f32, rgb[1] as f32, rgb[2] as f32]);
                [o[0] as f64, o[1] as f64, o[2] as f64]
            }
            GradeOp::Lut3D(l) => {
                let o = l.apply([rgb[0] as f32, rgb[1] as f32, rgb[2] as f32]);
                [o[0] as f64, o[1] as f64, o[2] as f64]
            }
            GradeOp::Matrix { m, offset } => {
                let v = m.apply(rgb);
                [v[0] + offset[0], v[1] + offset[1], v[2] + offset[2]]
            }
        }
    }

    /// Short label for UIs / CDL export reports.
    pub fn label(&self) -> &'static str {
        match self {
            GradeOp::Exposure { .. } => "exposure",
            GradeOp::Wheels(_) => "wheels",
            GradeOp::Cdl(_) => "cdl",
            GradeOp::Contrast { .. } => "contrast",
            GradeOp::Saturation { .. } => "saturation",
            GradeOp::HueRotate { .. } => "hue_rotate",
            GradeOp::TemperatureTint { .. } => "temperature_tint",
            GradeOp::PrinterLights { .. } => "printer_lights",
            GradeOp::Curves(_) => "curves",
            GradeOp::Lut3D(_) => "lut3d",
            GradeOp::Matrix { .. } => "matrix",
        }
    }
}

/// A layer: an optional geometric mask and/or HSL qualifier that limit where
/// the ops apply, blended by `opacity`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradeLayer {
    pub name: String,
    pub enabled: bool,
    pub opacity: f32,
    pub mask: Option<Mask>,
    pub qualifier: Option<HslQualifier>,
    pub ops: Vec<GradeOp>,
}

impl Default for GradeLayer {
    fn default() -> Self {
        GradeLayer {
            name: "layer".into(),
            enabled: true,
            opacity: 1.0,
            mask: None,
            qualifier: None,
            ops: Vec::new(),
        }
    }
}

impl GradeLayer {
    /// A primary (unmasked) layer.
    pub fn primary(name: &str, ops: Vec<GradeOp>) -> GradeLayer {
        GradeLayer {
            name: name.into(),
            ops,
            ..Default::default()
        }
    }

    /// True when the layer applies everywhere.
    pub fn is_primary(&self) -> bool {
        self.mask.is_none() && self.qualifier.is_none() && (self.opacity - 1.0).abs() < 1e-6
    }

    /// Apply the ops of this layer to one pixel (ignoring mask / qualifier).
    pub fn apply_ops(&self, rgb: [f64; 3], working: PrimariesId) -> [f64; 3] {
        let mut c = rgb;
        for op in &self.ops {
            c = op.apply(c, working);
        }
        c
    }
}

/// The complete grade of a clip: ordered layers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradeStack {
    /// Format version for forward compatibility.
    pub version: u32,
    /// Primaries of the working space the ops run in (informs temperature/tint).
    pub working_primaries: PrimariesId,
    pub layers: Vec<GradeLayer>,
}

impl Default for GradeStack {
    fn default() -> Self {
        GradeStack {
            version: 1,
            working_primaries: PrimariesId::AcesAp1,
            layers: Vec::new(),
        }
    }
}

/// Result of a best-effort CDL export.
#[derive(Clone, Debug, PartialEq)]
pub struct CdlExport {
    pub cdl: Cdl,
    /// True when some op could not be expressed and was skipped.
    pub lossy: bool,
    /// Labels of the skipped ops / layers.
    pub skipped: Vec<String>,
}

impl GradeStack {
    /// Empty stack.
    pub fn new() -> GradeStack {
        GradeStack::default()
    }

    /// Stack with one primary layer.
    pub fn primary(ops: Vec<GradeOp>) -> GradeStack {
        GradeStack {
            layers: vec![GradeLayer::primary("primary", ops)],
            ..Default::default()
        }
    }

    /// True when there is nothing to apply.
    pub fn is_empty(&self) -> bool {
        self.layers.iter().all(|l| !l.enabled || l.ops.is_empty())
    }

    /// Apply to one pixel (masks evaluated at normalized `uv`).
    pub fn apply_pixel(&self, rgb: [f64; 3], uv: [f32; 2]) -> [f64; 3] {
        let mut c = rgb;
        for layer in &self.layers {
            if !layer.enabled || layer.ops.is_empty() {
                continue;
            }
            let mut w = layer.opacity.clamp(0.0, 1.0);
            if let Some(m) = &layer.mask {
                w *= m.eval(uv);
            }
            if let Some(q) = &layer.qualifier {
                w *= q.weight([c[0] as f32, c[1] as f32, c[2] as f32]);
            }
            if w <= 0.0 {
                continue;
            }
            let g = layer.apply_ops(c, self.working_primaries);
            let w = w as f64;
            for i in 0..3 {
                c[i] += (g[i] - c[i]) * w;
            }
        }
        c
    }

    /// Apply in place to a `width × height` interleaved image (rayon).
    pub fn apply_image(&self, data: &mut [f32], width: usize, height: usize, channels: usize) {
        if self.is_empty() || channels < 3 || width == 0 || height == 0 {
            return;
        }
        // Rasterize masks once.
        let masks: Vec<Option<Vec<f32>>> = self
            .layers
            .iter()
            .map(|l| {
                if l.enabled {
                    l.mask.as_ref().map(|m| m.rasterize(width, height))
                } else {
                    None
                }
            })
            .collect();
        let wp = self.working_primaries;
        let row_len = width * channels;
        data.par_chunks_mut(row_len)
            .enumerate()
            .for_each(|(y, row)| {
                for (x, px) in row.chunks_exact_mut(channels).enumerate() {
                    let mut c = [px[0] as f64, px[1] as f64, px[2] as f64];
                    for (li, layer) in self.layers.iter().enumerate() {
                        if !layer.enabled || layer.ops.is_empty() {
                            continue;
                        }
                        let mut w = layer.opacity.clamp(0.0, 1.0);
                        if let Some(m) = &masks[li] {
                            w *= m[y * width + x];
                        }
                        if let Some(q) = &layer.qualifier {
                            w *= q.weight([c[0] as f32, c[1] as f32, c[2] as f32]);
                        }
                        if w <= 0.0 {
                            continue;
                        }
                        let g = layer.apply_ops(c, wp);
                        let w = w as f64;
                        for i in 0..3 {
                            c[i] += (g[i] - c[i]) * w;
                        }
                    }
                    px[0] = c[0] as f32;
                    px[1] = c[1] as f32;
                    px[2] = c[2] as f32;
                }
            });
    }

    /// Best-effort ASC CDL export of the primary (unmasked, full-opacity)
    /// layers. Exposure, wheels, contrast, saturation, printer lights and CDL
    /// ops are folded; everything else is reported as skipped.
    pub fn to_cdl_report(&self) -> CdlExport {
        let mut cdl = Cdl::default();
        let mut skipped = Vec::new();
        for layer in &self.layers {
            if !layer.enabled {
                continue;
            }
            if !layer.is_primary() {
                skipped.push(format!("layer `{}` (masked)", layer.name));
                continue;
            }
            for op in &layer.ops {
                match op {
                    GradeOp::Exposure { stops } => {
                        let k = 2f64.powf(*stops);
                        compose_linear(&mut cdl, [k; 3], [0.0; 3]);
                    }
                    GradeOp::PrinterLights { points } => {
                        compose_linear(&mut cdl, printer_lights_scale(*points), [0.0; 3]);
                    }
                    GradeOp::Contrast { amount, pivot } => {
                        // (in − p)·a + p  =  a·in + p·(1 − a)
                        compose_linear(&mut cdl, [*amount; 3], [pivot * (1.0 - amount); 3]);
                    }
                    GradeOp::Saturation { amount } => cdl.saturation *= *amount,
                    GradeOp::Wheels(w) => {
                        let (l, y, g, o) = w.effective();
                        // gain * (in*(1-lift) + lift)^(1/gamma) + offset
                        let mut slope = [0.0; 3];
                        let mut off = [0.0; 3];
                        for i in 0..3 {
                            slope[i] = 1.0 - l[i];
                            off[i] = l[i];
                        }
                        compose_linear(&mut cdl, slope, off);
                        let mut lossy = false;
                        for i in 0..3 {
                            if (cdl.power[i] - 1.0).abs() > 1e-9 && (y[i] - 1.0).abs() > 1e-9 {
                                lossy = true;
                            }
                            cdl.power[i] *= 1.0 / y[i];
                        }
                        if lossy {
                            skipped.push("wheels gamma composed approximately".into());
                        }
                        // post-power gain/offset: exact only if power == 1
                        if g.iter().any(|v| (v - 1.0).abs() > 1e-9)
                            || o.iter().any(|v| v.abs() > 1e-9)
                        {
                            if cdl.power.iter().any(|p| (p - 1.0).abs() > 1e-9) {
                                skipped.push(
                                    "wheels gain/offset after gamma folded approximately".into(),
                                );
                            }
                            compose_linear(&mut cdl, g, o);
                        }
                    }
                    GradeOp::Cdl(c) => {
                        compose_linear(&mut cdl, c.slope, c.offset);
                        let mut lossy = false;
                        for i in 0..3 {
                            if (cdl.power[i] - 1.0).abs() > 1e-9 && (c.power[i] - 1.0).abs() > 1e-9
                            {
                                lossy = true;
                            }
                            cdl.power[i] *= c.power[i];
                        }
                        if lossy {
                            skipped.push("cdl power composed approximately".into());
                        }
                        cdl.saturation *= c.saturation;
                    }
                    other => skipped.push(other.label().to_string()),
                }
            }
        }
        CdlExport {
            lossy: !skipped.is_empty(),
            cdl,
            skipped,
        }
    }

    /// Best-effort ASC CDL export (see [`GradeStack::to_cdl_report`]).
    pub fn to_cdl(&self) -> Cdl {
        self.to_cdl_report().cdl
    }
}

/// Fold `out = in * s + o` into a CDL. Exact when the CDL power is 1;
/// otherwise the linear part is folded before the power (approximate).
fn compose_linear(cdl: &mut Cdl, s: [f64; 3], o: [f64; 3]) {
    for i in 0..3 {
        if (cdl.power[i] - 1.0).abs() < 1e-9 {
            cdl.slope[i] *= s[i];
            cdl.offset[i] = cdl.offset[i] * s[i] + o[i];
        } else {
            // approximate: apply to the pre-power affine part
            cdl.slope[i] *= s[i].abs().powf(1.0 / cdl.power[i]) * s[i].signum();
            cdl.offset[i] += o[i];
        }
    }
}

// ---------------------------------------------------------------------------
// CDL XML
// ---------------------------------------------------------------------------

fn fmt3(v: [f64; 3]) -> String {
    format!(
        "{} {} {}",
        trim_float(v[0]),
        trim_float(v[1]),
        trim_float(v[2])
    )
}

fn trim_float(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s.is_empty() || s == "-0" {
        "0".into()
    } else {
        s
    }
}

/// Serialize a CDL as an ASC `<ColorCorrection>` XML document.
pub fn write_cdl_xml(cdl: &Cdl, id: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ColorCorrection id=\"{}\">\n  <SOPNode>\n    <Slope>{}</Slope>\n    <Offset>{}</Offset>\n    <Power>{}</Power>\n  </SOPNode>\n  <SatNode>\n    <Saturation>{}</Saturation>\n  </SatNode>\n</ColorCorrection>\n",
        xml_escape(id),
        fmt3(cdl.slope),
        fmt3(cdl.offset),
        fmt3(cdl.power),
        trim_float(cdl.saturation)
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Locate the first `<tag ...>` opening element at or after `from`
/// (a full tag name, so `<ColorCorrection` does not match
/// `<ColorCorrectionCollection`). Returns `(start of '<', index of '>')`.
fn find_open_tag(xml: &str, tag: &str, from: usize) -> Option<(usize, usize)> {
    let open = format!("<{tag}");
    let mut search = from;
    loop {
        let start = xml[search..].find(&open)? + search;
        let after = start + open.len();
        let next = xml[after..].chars().next()?;
        if next == '>' || next.is_whitespace() || next == '/' {
            let gt = xml[after..].find('>')? + after;
            return Some((start, gt));
        }
        search = after;
    }
}

/// Extract the text between `<tag ...>` and `</tag>` (first occurrence at or after `from`);
/// returns the body and the index just past the closing tag.
fn tag_text<'a>(xml: &'a str, tag: &str, from: usize) -> Option<(&'a str, usize)> {
    let (_, gt) = find_open_tag(xml, tag, from)?;
    let close = format!("</{tag}>");
    let end = xml[gt + 1..].find(&close)? + gt + 1;
    Some((&xml[gt + 1..end], end + close.len()))
}

fn attr_value(open_tag: &str, name: &str) -> Option<String> {
    let key = format!("{name}=");
    let i = open_tag.find(&key)? + key.len();
    let rest = &open_tag[i..];
    let q = rest.chars().next()?;
    if q != '"' && q != '\'' {
        return None;
    }
    let end = rest[1..].find(q)? + 1;
    Some(xml_unescape(&rest[1..end]))
}

fn parse3(s: &str, what: &str) -> Result<[f64; 3]> {
    let v: std::result::Result<Vec<f64>, _> =
        s.split_whitespace().map(|t| t.parse::<f64>()).collect();
    match v {
        Ok(v) if v.len() == 3 => Ok([v[0], v[1], v[2]]),
        _ => Err(Error::Parse {
            line: 0,
            message: format!("bad {what}: `{}`", s.trim()),
        }),
    }
}

/// Parse every `<ColorCorrection>` in an ASC CDL / CCC / CC document,
/// returning `(id, cdl)` pairs in document order.
pub fn parse_cdl_xml_all(xml: &str) -> Result<Vec<(String, Cdl)>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some((body, next)) = tag_text(xml, "ColorCorrection", pos) {
        let id = find_open_tag(xml, "ColorCorrection", pos)
            .and_then(|(start, gt)| attr_value(&xml[start..=gt], "id"))
            .unwrap_or_default();
        let mut cdl = Cdl::default();
        if let Some((sop, _)) = tag_text(body, "SOPNode", 0) {
            if let Some((s, _)) = tag_text(sop, "Slope", 0) {
                cdl.slope = parse3(s, "Slope")?;
            }
            if let Some((s, _)) = tag_text(sop, "Offset", 0) {
                cdl.offset = parse3(s, "Offset")?;
            }
            if let Some((s, _)) = tag_text(sop, "Power", 0) {
                cdl.power = parse3(s, "Power")?;
            }
        }
        if let Some((sat, _)) = tag_text(body, "SatNode", 0) {
            if let Some((s, _)) = tag_text(sat, "Saturation", 0) {
                cdl.saturation = s.trim().parse().map_err(|_| Error::Parse {
                    line: 0,
                    message: format!("bad Saturation `{}`", s.trim()),
                })?;
            }
        }
        out.push((id, cdl));
        pos = next;
    }
    if out.is_empty() {
        return Err(Error::Parse {
            line: 0,
            message: "no <ColorCorrection> element".into(),
        });
    }
    Ok(out)
}

/// Parse the first `<ColorCorrection>` of a CDL XML document.
pub fn parse_cdl_xml(xml: &str) -> Result<Cdl> {
    Ok(parse_cdl_xml_all(xml)?.remove(0).1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdl_formula() {
        let cdl = Cdl {
            slope: [1.2, 1.0, 0.8],
            offset: [0.01, 0.0, -0.01],
            power: [1.1, 1.0, 0.9],
            saturation: 1.0,
        };
        let o = cdl_apply([0.5, 0.5, 0.5], &cdl);
        assert!((o[0] - (0.5 * 1.2 + 0.01f64).powf(1.1)).abs() < 1e-12);
        assert!((o[1] - 0.5).abs() < 1e-12);
        assert!((o[2] - (0.5 * 0.8 - 0.01f64).powf(0.9)).abs() < 1e-12);
        let sat = Cdl {
            saturation: 0.5,
            ..Cdl::default()
        };
        let o = cdl_apply([1.0, 0.0, 0.0], &sat);
        let l = 0.2126;
        assert!((o[0] - (l + (1.0 - l) * 0.5)).abs() < 1e-12);
        assert!((o[1] - l * 0.5).abs() < 1e-12);
        let back = cdl_apply_inverse(cdl_apply([0.3, 0.6, 0.9], &cdl), &cdl);
        assert!((back[0] - 0.3).abs() < 1e-9 && (back[2] - 0.9).abs() < 1e-9);
    }

    #[test]
    fn wheels_formula() {
        let w = Wheels {
            lift: [0.1, 0.0, 0.0],
            gain: [1.0, 2.0, 1.0],
            gamma: [1.0, 1.0, 2.0],
            ..Default::default()
        };
        let o = w.apply([0.5, 0.5, 0.25]);
        assert!((o[0] - (0.5 + 0.1 * 0.5)).abs() < 1e-12);
        assert!((o[1] - 1.0).abs() < 1e-12);
        assert!((o[2] - 0.5).abs() < 1e-12);
        assert!(Wheels::default().is_identity());
    }

    #[test]
    fn hue_rotation_preserves_luma_and_rotates() {
        let luma = |c: [f64; 3]| LUMA_709[0] * c[0] + LUMA_709[1] * c[1] + LUMA_709[2] * c[2];
        let c = [0.8, 0.2, 0.2];
        let o = hue_rotate_apply(c, 120.0);
        assert!((luma(c) - luma(o)).abs() < 1e-9, "{c:?} -> {o:?}");
        assert!(
            (o[0] - c[0]).abs() > 0.1,
            "rotation must change the colour: {o:?}"
        );
        let m = hue_rotate_matrix(120.0).apply(c);
        for i in 0..3 {
            assert!((m[i] - o[i]).abs() < 1e-9);
        }
        let back = hue_rotate_apply(o, -120.0);
        for i in 0..3 {
            assert!((back[i] - c[i]).abs() < 1e-9);
        }
        // exact luma preservation on arbitrary colours and angles; neutrals are fixed points
        for (col, deg) in [
            ([0.1, 0.9, 0.3], 37.0),
            ([0.02, 0.05, 0.7], -200.0),
            ([1.2, 0.4, 0.0], 720.0),
        ] {
            let o = hue_rotate_apply(col, deg);
            assert!((luma(col) - luma(o)).abs() < 1e-9, "{col:?} @ {deg}: {o:?}");
        }
        let g = hue_rotate_apply([0.5; 3], 90.0);
        for v in g {
            assert!((v - 0.5).abs() < 1e-12, "{g:?}");
        }
        assert!(hue_rotate_matrix(0.0).is_identity(1e-12));
        assert!(hue_rotate_matrix(360.0).is_identity(1e-12));
    }

    #[test]
    fn temperature_warms() {
        let gray = [0.5, 0.5, 0.5];
        assert!(temperature_tint_matrix(0.0, 0.0, PrimariesId::Rec709).is_identity(1e-12));
        for working in [
            PrimariesId::Rec709,
            PrimariesId::AcesAp1,
            PrimariesId::ArriWideGamut4,
        ] {
            let warm = temperature_tint_matrix(1500.0, 0.0, working).apply(gray);
            assert!(warm[0] / warm[2] > 1.05, "warm: {warm:?}");
            assert!(
                warm[1] < warm[0] && warm[1] > warm[2],
                "warm gray must stay r > g > b: {warm:?}"
            );
            let warmer = temperature_tint_matrix(3000.0, 0.0, working).apply(gray);
            assert!(
                warmer[0] / warmer[2] > warm[0] / warm[2],
                "more shift → higher r/b: {warmer:?}"
            );
            let cool = temperature_tint_matrix(-1500.0, 0.0, working).apply(gray);
            assert!(cool[0] / cool[2] < 0.95, "cool: {cool:?}");
            assert!(
                cool[1] > cool[0] && cool[1] < cool[2],
                "cool gray must stay r < g < b: {cool:?}"
            );
            let magenta = temperature_tint_matrix(0.0, 50.0, working).apply(gray);
            assert!(
                magenta[1] < magenta[0] && magenta[1] < magenta[2],
                "magenta: {magenta:?}"
            );
            let green = temperature_tint_matrix(0.0, -50.0, working).apply(gray);
            assert!(
                green[1] > green[0] && green[1] > green[2],
                "green: {green:?}"
            );
        }
    }

    #[test]
    fn stack_apply_with_mask_and_qualifier() {
        let mut stack = GradeStack::primary(vec![GradeOp::Exposure { stops: 1.0 }]);
        stack.layers.push(GradeLayer {
            name: "left half red".into(),
            mask: Some(Mask::Rectangle {
                center: [0.25, 0.5],
                size: [0.5, 1.0],
                rotation_deg: 0.0,
                softness: 0.0,
                invert: false,
            }),
            ops: vec![GradeOp::Saturation { amount: 0.0 }],
            ..Default::default()
        });
        let (w, h) = (4, 2);
        let mut img = vec![0.0f32; w * h * 3];
        for px in img.chunks_mut(3) {
            px.copy_from_slice(&[0.4, 0.1, 0.1]);
        }
        stack.apply_image(&mut img, w, h, 3);
        // right half: exposure only
        assert!((img[3 * 3] - 0.8).abs() < 1e-6 && (img[3 * 3 + 1] - 0.2).abs() < 1e-6);
        // left half: exposure then desaturated
        assert!((img[0] - img[1]).abs() < 1e-6, "{:?}", &img[0..3]);
        let px = stack.apply_pixel([0.4, 0.1, 0.1], [0.9, 0.5]);
        assert!((px[0] - 0.8).abs() < 1e-9);
    }

    #[test]
    fn cdl_export_and_xml_round_trip() {
        let stack = GradeStack::primary(vec![
            GradeOp::Exposure { stops: 1.0 },
            GradeOp::Contrast {
                amount: 1.2,
                pivot: 0.18,
            },
            GradeOp::Saturation { amount: 0.9 },
        ]);
        let rep = stack.to_cdl_report();
        assert!(!rep.lossy);
        let cdl = rep.cdl;
        assert!((cdl.slope[0] - 2.4).abs() < 1e-9);
        assert!((cdl.offset[0] - 0.18 * (1.0 - 1.2)).abs() < 1e-9);
        assert!((cdl.saturation - 0.9).abs() < 1e-9);
        // numeric equivalence with the stack
        let x = [0.3, 0.5, 0.7];
        let a = stack.apply_pixel(x, [0.5, 0.5]);
        let b = cdl_apply(x, &cdl);
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < 1e-9, "{a:?} {b:?}");
        }
        let xml = write_cdl_xml(&cdl, "cc 01");
        let back = parse_cdl_xml(&xml).unwrap();
        assert!((back.slope[0] - cdl.slope[0]).abs() < 1e-6);
        assert!((back.saturation - 0.9).abs() < 1e-6);
        let all = parse_cdl_xml_all(&format!(
            "<ColorCorrectionCollection>{xml}{}</ColorCorrectionCollection>",
            write_cdl_xml(&Cdl::default(), "b")
        ))
        .unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].0, "cc 01");
        assert_eq!(all[1].0, "b");
        assert!(parse_cdl_xml("<nothing/>").is_err());
    }

    #[test]
    fn grade_stack_serde_round_trip() {
        let mut stack = GradeStack::primary(vec![
            GradeOp::Wheels(Wheels::default()),
            GradeOp::Curves(Curves::default()),
        ]);
        stack.layers.push(GradeLayer {
            qualifier: Some(HslQualifier::hue(120.0, 40.0, 5.0)),
            mask: Some(Mask::circle([0.5, 0.5], 0.3, 0.1)),
            ops: vec![GradeOp::HueRotate { degrees: 10.0 }],
            ..Default::default()
        });
        let json = serde_json::to_string(&stack).unwrap();
        let back: GradeStack = serde_json::from_str(&json).unwrap();
        assert_eq!(back, stack);
    }
}
