//! CPU mirrors of the pixel kernels. These are the reference the GPU path is
//! validated against, and the fallback when no adapter is available.

use crate::kernels::MergeOp;

pub type Rgba = [f32; 4];

pub fn luma709(c: &[f32]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Porter-Duff merge of `b` over `a` (both premultiplied).
pub fn merge(op: MergeOp, a: Rgba, b: Rgba) -> Rgba {
    let mut out = [0.0; 4];
    for i in 0..4 {
        let (pa, pb) = (a[i], b[i]);
        out[i] = match op {
            MergeOp::Over => pb + pa * (1.0 - b[3]),
            MergeOp::Under => pa + pb * (1.0 - a[3]),
            MergeOp::Plus => pa + pb,
            MergeOp::Minus => pa - pb,
            MergeOp::Multiply => pa * pb,
            MergeOp::Screen => pa + pb - pa * pb,
            MergeOp::Difference => (pa - pb).abs(),
            MergeOp::Max => pa.max(pb),
            MergeOp::Min => pa.min(pb),
            MergeOp::Mask => pa * b[3],
            MergeOp::Stencil => pa * (1.0 - b[3]),
            MergeOp::Xor => pa * (1.0 - b[3]) + pb * (1.0 - a[3]),
            MergeOp::In => pb * a[3],
            MergeOp::Out => pb * (1.0 - a[3]),
            MergeOp::Atop => pb * a[3] + pa * (1.0 - b[3]),
        };
    }
    out
}

pub fn merge_image(op: MergeOp, mix: f32, a: &[f32], b: &[f32], out: &mut [f32]) {
    let (a4, _) = a.as_chunks::<4>();
    let (b4, _) = b.as_chunks::<4>();
    let (o4, _) = out.as_chunks_mut::<4>();
    for ((pa, pb), po) in a4.iter().zip(b4.iter()).zip(o4.iter_mut()) {
        let m = merge(op, *pa, *pb);
        for i in 0..4 {
            po[i] = pa[i] + (m[i] - pa[i]) * mix;
        }
    }
}

/// ASC CDL on one linear RGB triple.
pub fn cdl(c: [f32; 3], slope: [f32; 3], offset: [f32; 3], power: [f32; 3], sat: f32) -> [f32; 3] {
    let mut sop = [0.0; 3];
    for i in 0..3 {
        sop[i] = (c[i] * slope[i] + offset[i]).max(0.0).powf(power[i]);
    }
    let l = luma709(&sop);
    let mut out = [0.0; 3];
    for i in 0..3 {
        out[i] = (l + sat * (sop[i] - l)).max(0.0);
    }
    out
}

/// Circle of confusion in pixels — mirrors `dof.wgsl::coc_px`.
pub fn coc_px(
    distance_m: f32,
    focal_mm: f32,
    f_stop: f32,
    focus_m: f32,
    sensor_w_mm: f32,
    width_px: u32,
) -> f32 {
    let f = focal_mm / 1000.0;
    let aperture = f / f_stop;
    let c = aperture * (distance_m - focus_m).abs() / distance_m.max(1e-4)
        * (f / (focus_m - f).max(1e-4));
    c / (sensor_w_mm / 1000.0) * width_px as f32
}

/// Chroma key alpha — mirrors `keyer.wgsl`.
pub fn chroma_key_alpha(c: [f32; 3], key: [f32; 3], tolerance: f32, softness: f32) -> f32 {
    fn cbcr(c: [f32; 3]) -> (f32, f32) {
        let l = luma709(&c);
        ((c[2] - l) / 1.8556, (c[0] - l) / 1.5748)
    }
    let (cb, cr) = cbcr(c);
    let (kb, kr) = cbcr(key);
    let d = ((cb - kb).powi(2) + (cr - kr).powi(2)).sqrt();
    smoothstep(tolerance, tolerance + softness, d)
}

pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Histogram of luma into 256 bins.
pub fn histogram_luma(rgba: &[f32]) -> [u32; 256] {
    let mut h = [0u32; 256];
    for p in rgba.as_chunks::<4>().0 {
        let l = luma709(p).clamp(0.0, 1.0);
        h[(l * 255.0 + 0.5) as usize] += 1;
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_matches_porter_duff() {
        let a = [0.0, 0.0, 1.0, 1.0];
        let b = [0.5, 0.0, 0.0, 0.5];
        let o = merge(MergeOp::Over, a, b);
        assert_eq!(o, [0.5, 0.0, 0.5, 1.0]);
        let m = merge(MergeOp::Mask, a, b);
        assert_eq!(m, [0.0, 0.0, 0.5, 0.5]);
    }

    #[test]
    fn cdl_identity_and_saturation() {
        let c = [0.2, 0.4, 0.6];
        assert_eq!(cdl(c, [1.0; 3], [0.0; 3], [1.0; 3], 1.0), c);
        let g = cdl(c, [1.0; 3], [0.0; 3], [1.0; 3], 0.0);
        assert!(
            (g[0] - g[1]).abs() < 1e-6 && (g[1] - g[2]).abs() < 1e-6,
            "zero saturation is gray"
        );
    }

    #[test]
    fn coc_is_zero_at_focus_and_grows_away() {
        let at = coc_px(3.0, 50.0, 2.0, 3.0, 36.0, 1920);
        let near = coc_px(1.5, 50.0, 2.0, 3.0, 36.0, 1920);
        let far = coc_px(10.0, 50.0, 2.0, 3.0, 36.0, 1920);
        assert!(at.abs() < 1e-6);
        assert!(near > 1.0 && far > 1.0);
    }

    #[test]
    fn key_alpha_zero_on_key_color() {
        let key = [0.1, 0.8, 0.1];
        assert_eq!(chroma_key_alpha(key, key, 0.1, 0.1), 0.0);
        assert_eq!(chroma_key_alpha([0.8, 0.1, 0.1], key, 0.1, 0.1), 1.0);
    }
}
