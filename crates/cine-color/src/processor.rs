//! Color transform chains: [`Op`], [`Transform`] and the optimized [`Processor`].
//!
//! A processor is an ordered list of per-pixel operations. Building one
//! concatenates adjacent matrices, drops identities and cancels a transfer
//! decode immediately followed by the same encode, so a typical
//! `camera log → ACEScg → display` chain collapses to
//! `decode · matrix · encode`.

#![allow(clippy::needless_range_loop)]

use crate::grade::{cdl_apply, cdl_is_identity};
use crate::lut::{Lut1D, Lut3D};
use crate::space::{rgb_to_rgb_matrix, ColorSpace, Mat3};
use crate::transfer::Transfer;
use rayon::prelude::*;
use std::fmt;
use std::sync::Arc;
use ucg_schema::Cdl;

/// Type of a user supplied per-pixel function.
pub type PixelFn = Arc<dyn Fn([f64; 3]) -> [f64; 3] + Send + Sync>;

/// Direction of an invertible op.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Forward,
    Inverse,
}

impl Direction {
    /// Flip.
    pub fn flipped(self) -> Direction {
        match self {
            Direction::Forward => Direction::Inverse,
            Direction::Inverse => Direction::Forward,
        }
    }
}

/// Parameters of an OCIO-style log/lin conversion (`LogAffineTransform`,
/// `LogCameraTransform`). Forward direction is **linear → log**.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LogParams {
    pub base: f64,
    pub log_side_slope: [f64; 3],
    pub log_side_offset: [f64; 3],
    pub lin_side_slope: [f64; 3],
    pub lin_side_offset: [f64; 3],
    /// Linear-segment breakpoint (camera log); `None` for a pure log-affine curve.
    pub lin_side_break: Option<[f64; 3]>,
    /// Slope of the linear segment; when `None` the curve is C¹ continuous at the break.
    pub linear_slope: Option<[f64; 3]>,
}

impl Default for LogParams {
    fn default() -> Self {
        LogParams {
            base: 2.0,
            log_side_slope: [1.0; 3],
            log_side_offset: [0.0; 3],
            lin_side_slope: [1.0; 3],
            lin_side_offset: [0.0; 3],
            lin_side_break: None,
            linear_slope: None,
        }
    }
}

impl LogParams {
    fn slope_at_break(&self, c: usize, brk: f64) -> f64 {
        self.log_side_slope[c] * self.lin_side_slope[c]
            / ((self.lin_side_slope[c] * brk + self.lin_side_offset[c]) * self.base.ln())
    }

    fn log_of_lin(&self, c: usize, x: f64) -> f64 {
        let arg = self.lin_side_slope[c] * x + self.lin_side_offset[c];
        self.log_side_slope[c] * arg.max(1e-30).ln() / self.base.ln() + self.log_side_offset[c]
    }

    /// Linear → log for one channel.
    pub fn lin_to_log(&self, c: usize, x: f64) -> f64 {
        if let Some(brk) = self.lin_side_break {
            let b = brk[c];
            if x < b {
                let slope = self
                    .linear_slope
                    .map(|s| s[c])
                    .unwrap_or_else(|| self.slope_at_break(c, b));
                let log_at_break = self.log_of_lin(c, b);
                return slope * (x - b) + log_at_break;
            }
        }
        self.log_of_lin(c, x)
    }

    /// Log → linear for one channel.
    pub fn log_to_lin(&self, c: usize, y: f64) -> f64 {
        if let Some(brk) = self.lin_side_break {
            let b = brk[c];
            let log_at_break = self.log_of_lin(c, b);
            if y < log_at_break {
                let slope = self
                    .linear_slope
                    .map(|s| s[c])
                    .unwrap_or_else(|| self.slope_at_break(c, b));
                if slope.abs() < 1e-30 {
                    return b;
                }
                return (y - log_at_break) / slope + b;
            }
        }
        let e = (y - self.log_side_offset[c]) / self.log_side_slope[c];
        (self.base.powf(e) - self.lin_side_offset[c]) / self.lin_side_slope[c]
    }
}

/// One per-pixel operation.
#[derive(Clone)]
pub enum Op {
    /// `out = m * in + offset`.
    Matrix { m: Mat3, offset: [f64; 3] },
    /// Apply a transfer function. `decode` = code → linear, else linear → code.
    Transfer { func: Transfer, decode: bool },
    /// 1D LUT.
    Lut1D(Arc<Lut1D>),
    /// 3D LUT.
    Lut3D(Arc<Lut3D>),
    /// ASC CDL (forward or inverse).
    Cdl { cdl: Cdl, direction: Direction },
    /// Linear range remap with optional clamping to the output range.
    Range {
        min_in: Option<f64>,
        max_in: Option<f64>,
        min_out: Option<f64>,
        max_out: Option<f64>,
        clamp: bool,
    },
    /// Per-channel power (`out = in^value`; negatives clamp to 0).
    Exponent {
        value: [f64; 3],
        direction: Direction,
    },
    /// Parametric log/lin curve; forward = linear → log.
    Log {
        params: LogParams,
        direction: Direction,
    },
    /// Saturation about Rec.709 luma.
    Saturation(f64),
    /// OCIO `ExponentWithLinearTransform` ("moncurve": power with a linear
    /// toe, e.g. gamma 2.4 / offset 0.055 = sRGB). Forward = code → linear.
    MonCurve {
        gamma: [f64; 3],
        offset: [f64; 3],
        direction: Direction,
    },
    /// Arbitrary closure (not serializable; invertible only when `inv` is given).
    Custom {
        name: String,
        f: PixelFn,
        inv: Option<PixelFn>,
    },
}

impl fmt::Debug for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Op::Matrix { m, offset } => write!(f, "Matrix({:?}, {:?})", m.0, offset),
            Op::Transfer { func, decode } => write!(f, "Transfer({func:?}, decode={decode})"),
            Op::Lut1D(l) => write!(f, "Lut1D(size={})", l.size),
            Op::Lut3D(l) => write!(f, "Lut3D(size={})", l.size),
            Op::Cdl { cdl, direction } => write!(f, "Cdl({cdl:?}, {direction:?})"),
            Op::Range {
                min_in,
                max_in,
                min_out,
                max_out,
                clamp,
            } => {
                write!(
                    f,
                    "Range({min_in:?},{max_in:?} -> {min_out:?},{max_out:?}, clamp={clamp})"
                )
            }
            Op::Exponent { value, direction } => write!(f, "Exponent({value:?}, {direction:?})"),
            Op::Log { params, direction } => write!(f, "Log({params:?}, {direction:?})"),
            Op::Saturation(s) => write!(f, "Saturation({s})"),
            Op::MonCurve {
                gamma,
                offset,
                direction,
            } => write!(f, "MonCurve({gamma:?}, {offset:?}, {direction:?})"),
            Op::Custom { name, inv, .. } => {
                write!(f, "Custom({name}, invertible={})", inv.is_some())
            }
        }
    }
}

impl Op {
    /// A plain matrix with zero offset.
    pub fn matrix(m: Mat3) -> Op {
        Op::Matrix {
            m,
            offset: [0.0; 3],
        }
    }

    /// Decode a transfer function (code → linear).
    pub fn decode(func: Transfer) -> Op {
        Op::Transfer { func, decode: true }
    }

    /// Encode a transfer function (linear → code).
    pub fn encode(func: Transfer) -> Op {
        Op::Transfer {
            func,
            decode: false,
        }
    }

    /// Multiply every channel by `s`.
    pub fn scale(s: f64) -> Op {
        Op::matrix(Mat3::diag([s; 3]))
    }

    /// Clamp to `[lo, hi]`.
    pub fn clamp(lo: f64, hi: f64) -> Op {
        Op::Range {
            min_in: Some(lo),
            max_in: Some(hi),
            min_out: Some(lo),
            max_out: Some(hi),
            clamp: true,
        }
    }

    /// Wrap a closure.
    pub fn custom(name: &str, f: impl Fn([f64; 3]) -> [f64; 3] + Send + Sync + 'static) -> Op {
        Op::Custom {
            name: name.to_string(),
            f: Arc::new(f),
            inv: None,
        }
    }

    /// Wrap a closure together with its inverse.
    pub fn custom_invertible(
        name: &str,
        f: impl Fn([f64; 3]) -> [f64; 3] + Send + Sync + 'static,
        inv: impl Fn([f64; 3]) -> [f64; 3] + Send + Sync + 'static,
    ) -> Op {
        Op::Custom {
            name: name.to_string(),
            f: Arc::new(f),
            inv: Some(Arc::new(inv)),
        }
    }

    /// True when the op is (numerically) the identity.
    pub fn is_identity(&self) -> bool {
        let one = |v: &f64| (v - 1.0).abs() < 1e-12;
        let zero = |v: &f64| v.abs() < 1e-12;
        match self {
            Op::Matrix { m, offset } => m.is_identity(1e-12) && offset.iter().all(zero),
            Op::Transfer {
                func: Transfer::Linear,
                ..
            } => true,
            Op::Transfer {
                func: Transfer::Gamma(g),
                ..
            } => one(g),
            Op::Saturation(s) => one(s),
            Op::Exponent { value, .. } => value.iter().all(one),
            Op::MonCurve { gamma, offset, .. } => gamma.iter().all(one) && offset.iter().all(zero),
            Op::Cdl { cdl, .. } => cdl_is_identity(cdl),
            // A range that neither rescales nor clamps.
            Op::Range {
                min_in,
                max_in,
                min_out,
                max_out,
                clamp,
            } => {
                let same = |a: &Option<f64>, b: &Option<f64>| match (a, b) {
                    (Some(a), Some(b)) => (a - b).abs() < 1e-12,
                    (None, None) => true,
                    _ => false,
                };
                !clamp && same(min_in, min_out) && same(max_in, max_out)
            }
            _ => false,
        }
    }

    /// The inverse op, when one exists analytically.
    pub fn inverse(&self) -> Option<Op> {
        Some(match self {
            Op::Matrix { m, offset } => {
                let inv = m.inverse()?;
                let o = inv.apply(*offset);
                Op::Matrix {
                    m: inv,
                    offset: [-o[0], -o[1], -o[2]],
                }
            }
            Op::Transfer { func, decode } => Op::Transfer {
                func: *func,
                decode: !decode,
            },
            Op::Cdl { cdl, direction } => Op::Cdl {
                cdl: cdl.clone(),
                direction: direction.flipped(),
            },
            // As in OCIO, the inverse of a range swaps its input and output
            // bounds and keeps the clamp; for a clamping range this is a
            // pseudo-inverse (exact wherever the forward did not clamp).
            Op::Range {
                min_in,
                max_in,
                min_out,
                max_out,
                clamp,
            } => {
                if *clamp && min_in.is_none() && max_in.is_none() {
                    // pure output clamp: idempotent, its own pseudo-inverse
                    return Some(self.clone());
                }
                Op::Range {
                    min_in: *min_out,
                    max_in: *max_out,
                    min_out: *min_in,
                    max_out: *max_in,
                    clamp: *clamp,
                }
            }
            Op::Exponent { value, direction } => Op::Exponent {
                value: *value,
                direction: direction.flipped(),
            },
            Op::Log { params, direction } => Op::Log {
                params: params.clone(),
                direction: direction.flipped(),
            },
            Op::Saturation(s) => {
                if s.abs() < 1e-12 {
                    return None;
                }
                Op::Saturation(1.0 / s)
            }
            Op::MonCurve {
                gamma,
                offset,
                direction,
            } => Op::MonCurve {
                gamma: *gamma,
                offset: *offset,
                direction: direction.flipped(),
            },
            Op::Custom { name, f, inv } => {
                let i = inv.as_ref()?;
                Op::Custom {
                    name: format!("{name}⁻¹"),
                    f: i.clone(),
                    inv: Some(f.clone()),
                }
            }
            Op::Lut1D(_) | Op::Lut3D(_) => return None,
        })
    }

    /// Apply to one pixel (f64 precision).
    pub fn apply(&self, c: [f64; 3]) -> [f64; 3] {
        match self {
            Op::Matrix { m, offset } => {
                let v = m.apply(c);
                [v[0] + offset[0], v[1] + offset[1], v[2] + offset[2]]
            }
            Op::Transfer { func, decode } => {
                if *decode {
                    [func.decode(c[0]), func.decode(c[1]), func.decode(c[2])]
                } else {
                    [func.encode(c[0]), func.encode(c[1]), func.encode(c[2])]
                }
            }
            Op::Lut1D(l) => {
                let o = l.apply([c[0] as f32, c[1] as f32, c[2] as f32]);
                [o[0] as f64, o[1] as f64, o[2] as f64]
            }
            Op::Lut3D(l) => {
                let o = l.apply([c[0] as f32, c[1] as f32, c[2] as f32]);
                [o[0] as f64, o[1] as f64, o[2] as f64]
            }
            Op::Cdl { cdl, direction } => match direction {
                Direction::Forward => cdl_apply(c, cdl),
                Direction::Inverse => crate::grade::cdl_apply_inverse(c, cdl),
            },
            Op::Range {
                min_in,
                max_in,
                min_out,
                max_out,
                clamp,
            } => {
                let mut out = c;
                let scale = match (min_in, max_in, min_out, max_out) {
                    (Some(a), Some(b), Some(x), Some(y)) if (b - a).abs() > 1e-30 => {
                        Some((a, (y - x) / (b - a), x))
                    }
                    _ => None,
                };
                for v in out.iter_mut() {
                    if let Some((a, s, x)) = scale {
                        *v = (*v - a) * s + x;
                    }
                    if *clamp {
                        if let Some(lo) = min_out {
                            *v = v.max(*lo);
                        }
                        if let Some(hi) = max_out {
                            *v = v.min(*hi);
                        }
                    }
                }
                out
            }
            Op::Exponent { value, direction } => {
                let mut out = [0.0; 3];
                for i in 0..3 {
                    let p = match direction {
                        Direction::Forward => value[i],
                        Direction::Inverse => 1.0 / value[i],
                    };
                    out[i] = c[i].max(0.0).powf(p);
                }
                out
            }
            Op::Log { params, direction } => {
                let mut out = [0.0; 3];
                for i in 0..3 {
                    out[i] = match direction {
                        Direction::Forward => params.lin_to_log(i, c[i]),
                        Direction::Inverse => params.log_to_lin(i, c[i]),
                    };
                }
                out
            }
            Op::Saturation(s) => {
                let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                [l + (c[0] - l) * s, l + (c[1] - l) * s, l + (c[2] - l) * s]
            }
            Op::MonCurve {
                gamma,
                offset,
                direction,
            } => {
                let mut out = [0.0; 3];
                for i in 0..3 {
                    out[i] = moncurve(c[i], gamma[i], offset[i], *direction);
                }
                out
            }
            Op::Custom { f, .. } => f(c),
        }
    }
}

/// OCIO moncurve: `direction` forward maps encoded → linear.
fn moncurve(x: f64, gamma: f64, offset: f64, direction: Direction) -> f64 {
    if offset <= 0.0 || gamma <= 1.0 {
        // degenerate: plain power
        return match direction {
            Direction::Forward => x.max(0.0).powf(gamma),
            Direction::Inverse => x.max(0.0).powf(1.0 / gamma),
        };
    }
    let x_break = offset / (gamma - 1.0);
    let slope = ((gamma - 1.0) / offset)
        * ((offset * gamma) / ((gamma - 1.0) * (1.0 + offset))).powf(gamma);
    match direction {
        Direction::Forward => {
            if x >= x_break {
                ((x + offset) / (1.0 + offset)).powf(gamma)
            } else {
                x * slope
            }
        }
        Direction::Inverse => {
            let y_break = ((x_break + offset) / (1.0 + offset)).powf(gamma);
            if x >= y_break {
                (1.0 + offset) * x.max(0.0).powf(1.0 / gamma) - offset
            } else {
                x / slope
            }
        }
    }
}

/// An unoptimized, named chain of ops (what a config or a grade describes).
#[derive(Clone, Debug, Default)]
pub struct Transform {
    pub name: String,
    pub ops: Vec<Op>,
}

impl Transform {
    /// Empty transform.
    pub fn new(name: &str) -> Self {
        Transform {
            name: name.to_string(),
            ops: Vec::new(),
        }
    }

    /// Append an op (builder style).
    pub fn then(mut self, op: Op) -> Self {
        self.ops.push(op);
        self
    }

    /// Append all ops of another transform.
    pub fn extend(&mut self, other: Transform) {
        self.ops.extend(other.ops);
    }

    /// Inverse chain (reversed, each op inverted); `None` if any op is not invertible.
    pub fn inverse(&self) -> Option<Transform> {
        let mut ops = Vec::with_capacity(self.ops.len());
        for op in self.ops.iter().rev() {
            ops.push(op.inverse()?);
        }
        Some(Transform {
            name: format!("{}⁻¹", self.name),
            ops,
        })
    }

    /// Compile into an optimized processor.
    pub fn processor(&self) -> Processor {
        Processor::new(self.ops.clone())
    }
}

/// An optimized chain of ops ready to apply to pixels and images.
#[derive(Clone, Debug, Default)]
pub struct Processor {
    ops: Vec<Op>,
}

impl Processor {
    /// Build and optimize.
    pub fn new(ops: Vec<Op>) -> Processor {
        Processor { ops: optimize(ops) }
    }

    /// The identity processor.
    pub fn identity() -> Processor {
        Processor { ops: Vec::new() }
    }

    /// Optimized ops (for inspection / GPU mirroring).
    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    /// True when nothing would change: the (optimized) chain is empty or
    /// made only of identity ops.
    pub fn is_identity(&self) -> bool {
        self.ops.iter().all(Op::is_identity)
    }

    /// Chain another processor after this one.
    pub fn then(&self, next: &Processor) -> Processor {
        let mut ops = self.ops.clone();
        ops.extend(next.ops.iter().cloned());
        Processor::new(ops)
    }

    /// Conversion between two fully described colorspaces: decode the source
    /// transfer, convert primaries (Bradford adapted), encode the destination.
    pub fn from_to(src: &ColorSpace, dst: &ColorSpace) -> Processor {
        let mut ops = Vec::new();
        if src.transfer != Transfer::Linear {
            ops.push(Op::decode(src.transfer));
        }
        ops.push(Op::matrix(rgb_to_rgb_matrix(
            &src.primaries,
            &dst.primaries,
        )));
        if dst.transfer != Transfer::Linear {
            ops.push(Op::encode(dst.transfer));
        }
        Processor::new(ops)
    }

    /// Apply to one RGB pixel.
    pub fn apply_rgb(&self, rgb: &[f32; 3]) -> [f32; 3] {
        let mut c = [rgb[0] as f64, rgb[1] as f64, rgb[2] as f64];
        for op in &self.ops {
            c = op.apply(c);
        }
        [c[0] as f32, c[1] as f32, c[2] as f32]
    }

    /// Apply to one pixel in f64.
    pub fn apply_f64(&self, rgb: [f64; 3]) -> [f64; 3] {
        let mut c = rgb;
        for op in &self.ops {
            c = op.apply(c);
        }
        c
    }

    /// Apply in place to an interleaved image with `channels` ≥ 3 channels per
    /// pixel (extra channels such as alpha are untouched). Runs on the rayon pool.
    pub fn apply_image(&self, data: &mut [f32], channels: usize) {
        if self.ops.is_empty() || channels < 3 {
            return;
        }
        let chunk = channels * 4096;
        data.par_chunks_mut(chunk).for_each(|block| {
            for px in block.chunks_exact_mut(channels) {
                let o = self.apply_rgb(&[px[0], px[1], px[2]]);
                px[0] = o[0];
                px[1] = o[1];
                px[2] = o[2];
            }
        });
    }

    /// Inverse processor if every op is invertible.
    pub fn inverse(&self) -> Option<Processor> {
        let mut ops = Vec::with_capacity(self.ops.len());
        for op in self.ops.iter().rev() {
            ops.push(op.inverse()?);
        }
        Some(Processor::new(ops))
    }
}

/// Concatenate adjacent matrices, drop identities and cancel transfer pairs.
fn optimize(ops: Vec<Op>) -> Vec<Op> {
    let mut out: Vec<Op> = Vec::with_capacity(ops.len());
    for op in ops {
        if op.is_identity() {
            continue;
        }
        if let Some(last) = out.last() {
            match (last, &op) {
                (Op::Matrix { m: m1, offset: o1 }, Op::Matrix { m: m2, offset: o2 }) => {
                    let m = m2.mul(m1);
                    let t = m2.apply(*o1);
                    let offset = [t[0] + o2[0], t[1] + o2[1], t[2] + o2[2]];
                    out.pop();
                    let merged = Op::Matrix { m, offset };
                    if !merged.is_identity() {
                        out.push(merged);
                    }
                    continue;
                }
                (
                    Op::Transfer {
                        func: f1,
                        decode: d1,
                    },
                    Op::Transfer {
                        func: f2,
                        decode: d2,
                    },
                ) if f1 == f2 && d1 != d2 => {
                    out.pop();
                    continue;
                }
                (Op::Saturation(a), Op::Saturation(b)) => {
                    let s = a * b;
                    out.pop();
                    if (s - 1.0).abs() > 1e-12 {
                        out.push(Op::Saturation(s));
                    }
                    continue;
                }
                _ => {}
            }
        }
        out.push(op);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::Primaries;

    #[test]
    fn matrices_concatenate() {
        let p = Processor::new(vec![
            Op::matrix(Mat3::diag([2.0, 2.0, 2.0])),
            Op::Matrix {
                m: Mat3::IDENTITY,
                offset: [1.0, 0.0, 0.0],
            },
            Op::matrix(Mat3::diag([0.5, 0.5, 0.5])),
        ]);
        assert_eq!(p.ops().len(), 1);
        let o = p.apply_rgb(&[1.0, 1.0, 1.0]);
        assert!((o[0] - 1.5).abs() < 1e-6 && (o[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn transfer_pairs_cancel() {
        let p = Processor::new(vec![Op::decode(Transfer::SRgb), Op::encode(Transfer::SRgb)]);
        assert!(p.is_identity());
    }

    #[test]
    fn clamp_is_its_own_pseudo_inverse_and_identities_are_detected() {
        let clamp = Op::clamp(0.0, 1.0);
        let inv = clamp
            .inverse()
            .expect("clamping range has a pseudo-inverse");
        assert!(matches!(inv, Op::Range { clamp: true, .. }));
        let o = inv.apply([-0.5, 0.5, 1.5]);
        assert_eq!(o, [0.0, 0.5, 1.0]);
        let p = Processor::new(vec![
            Op::matrix(Mat3::diag([2.0; 3])),
            clamp,
            Op::encode(Transfer::SRgb),
        ]);
        let back = p.inverse().unwrap().apply_f64(p.apply_f64([0.1, 0.2, 0.3]));
        assert!(
            (back[0] - 0.1).abs() < 1e-12 && (back[2] - 0.3).abs() < 1e-12,
            "{back:?}"
        );
        assert!(Processor::identity().is_identity());
        assert!(Processor::new(vec![
            Op::Matrix {
                m: Mat3::IDENTITY,
                offset: [0.0; 3]
            },
            Op::Range {
                min_in: Some(0.0),
                max_in: Some(1.0),
                min_out: Some(0.0),
                max_out: Some(1.0),
                clamp: false
            },
            Op::Cdl {
                cdl: Cdl::default(),
                direction: Direction::Forward
            },
            Op::MonCurve {
                gamma: [1.0; 3],
                offset: [0.0; 3],
                direction: Direction::Forward
            },
        ])
        .is_identity());
        assert!(!Processor::new(vec![Op::clamp(0.0, 1.0)]).is_identity());
    }

    #[test]
    fn from_to_round_trip() {
        let a = ColorSpace::builtin("ARRI LogC4").unwrap();
        let b = ColorSpace::builtin("ACEScct").unwrap();
        let fwd = Processor::from_to(&a, &b);
        let back = Processor::from_to(&b, &a);
        let v = [0.3, 0.45, 0.6];
        let o = back.apply_rgb(&fwd.apply_rgb(&v));
        for i in 0..3 {
            assert!((o[i] - v[i]).abs() < 1e-4, "{o:?}");
        }
    }

    #[test]
    fn same_primaries_gives_no_matrix() {
        let a = ColorSpace::new("lin", Primaries::REC709, Transfer::Linear);
        let b = ColorSpace::new("srgb", Primaries::REC709, Transfer::SRgb);
        let p = Processor::from_to(&a, &b);
        assert_eq!(p.ops().len(), 1);
    }

    #[test]
    fn inverse_processor() {
        let p = Processor::new(vec![
            Op::matrix(Primaries::ACES_AP1.rgb_to_xyz()),
            Op::Exponent {
                value: [2.2; 3],
                direction: Direction::Forward,
            },
            Op::Saturation(0.8),
        ]);
        let inv = p.inverse().unwrap();
        let v = [0.2, 0.4, 0.6];
        let o = inv.apply_rgb(&p.apply_rgb(&v));
        for i in 0..3 {
            assert!((o[i] - v[i]).abs() < 1e-4, "{o:?}");
        }
    }

    #[test]
    fn log_camera_params_continuous() {
        let params = LogParams {
            base: 2.0,
            log_side_slope: [0.25; 3],
            log_side_offset: [0.4; 3],
            lin_side_slope: [1.0; 3],
            lin_side_offset: [0.01; 3],
            lin_side_break: Some([0.02; 3]),
            linear_slope: None,
        };
        let a = params.lin_to_log(0, 0.02 - 1e-9);
        let b = params.lin_to_log(0, 0.02 + 1e-9);
        assert!((a - b).abs() < 1e-6);
        for x in [-0.01, 0.0, 0.01, 0.1, 1.0, 8.0] {
            let back = params.log_to_lin(0, params.lin_to_log(0, x));
            assert!((back - x).abs() < 1e-9);
        }
    }

    #[test]
    fn moncurve_matches_srgb() {
        let op = Op::MonCurve {
            gamma: [2.4; 3],
            offset: [0.055; 3],
            direction: Direction::Forward,
        };
        for i in 0..=100 {
            let x = i as f64 / 100.0;
            let want = Transfer::SRgb.decode(x);
            let got = op.apply([x; 3])[0];
            assert!((got - want).abs() < 1e-4, "{x}: {got} vs {want}");
            let back = op.inverse().unwrap().apply([got; 3])[0];
            assert!((back - x).abs() < 1e-9);
        }
    }

    #[test]
    fn apply_image_parallel_matches_pixel() {
        let p = Processor::new(vec![Op::encode(Transfer::SRgb)]);
        let mut img: Vec<f32> = (0..(4 * 5000)).map(|i| (i % 97) as f32 / 97.0).collect();
        let copy = img.clone();
        p.apply_image(&mut img, 4);
        for (i, px) in copy.chunks(4).enumerate() {
            let want = p.apply_rgb(&[px[0], px[1], px[2]]);
            assert!((img[i * 4] - want[0]).abs() < 1e-6);
            assert_eq!(img[i * 4 + 3], px[3]);
        }
    }
}
