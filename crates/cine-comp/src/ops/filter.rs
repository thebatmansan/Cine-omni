//! Convolution-style filters: separable blur, unsharp mask, alpha
//! erode/dilate and edge blur. Rows are processed in parallel with rayon.

use crate::graph_model::BlurKind;
use crate::image::Image;
use rayon::prelude::*;

/// Normalized 1D Gaussian kernel with radius `ceil(3 sigma)`.
pub fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    if sigma <= 0.0 {
        return vec![1.0];
    }
    let radius = (sigma * 3.0).ceil() as i64;
    let mut k: Vec<f32> = (-radius..=radius)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    k
}

/// Normalized 1D box kernel with the given radius.
pub fn box_kernel(radius: usize) -> Vec<f32> {
    let n = 2 * radius + 1;
    vec![1.0 / n as f32; n]
}

/// Kernel radius in pixels for a blur of `size` (sigma for Gaussian, radius for box).
pub fn blur_radius(size: f32, kind: BlurKind) -> usize {
    match kind {
        BlurKind::Gaussian => (size.max(0.0) * 3.0).ceil() as usize,
        BlurKind::Box => size.max(0.0).round() as usize,
    }
}

/// Horizontal convolution with an odd, normalized kernel (edge clamped).
pub fn convolve_h(src: &Image, kernel: &[f32]) -> Image {
    let nc = src.nc();
    let w = src.width;
    let r = (kernel.len() / 2) as i64;
    let mut out = src.blank_like(src.channels.clone());
    if src.is_empty() {
        return out;
    }
    out.data
        .par_chunks_exact_mut(w * nc)
        .zip(src.data.par_chunks_exact(w * nc))
        .for_each(|(orow, srow)| {
            for (x, opx) in orow.chunks_exact_mut(nc).enumerate() {
                for (k, wt) in kernel.iter().enumerate() {
                    let sx = (x as i64 + k as i64 - r).clamp(0, w as i64 - 1) as usize;
                    let spx = &srow[sx * nc..sx * nc + nc];
                    for (o, s) in opx.iter_mut().zip(spx) {
                        *o += s * wt;
                    }
                }
            }
        });
    out
}

/// Vertical convolution with an odd, normalized kernel (edge clamped).
pub fn convolve_v(src: &Image, kernel: &[f32]) -> Image {
    let nc = src.nc();
    let w = src.width;
    let h = src.height;
    let r = (kernel.len() / 2) as i64;
    let mut out = src.blank_like(src.channels.clone());
    if src.is_empty() {
        return out;
    }
    let stride = w * nc;
    out.data
        .par_chunks_exact_mut(stride)
        .enumerate()
        .for_each(|(y, orow)| {
            for (k, wt) in kernel.iter().enumerate() {
                let sy = (y as i64 + k as i64 - r).clamp(0, h as i64 - 1) as usize;
                let srow = &src.data[sy * stride..sy * stride + stride];
                for (o, s) in orow.iter_mut().zip(srow) {
                    *o += s * wt;
                }
            }
        });
    out
}

/// Separable convolution (horizontal then vertical).
pub fn convolve_separable(src: &Image, kernel: &[f32]) -> Image {
    if kernel.len() <= 1 {
        return src.clone();
    }
    convolve_v(&convolve_h(src, kernel), kernel)
}

/// Blur with a Gaussian (`size` = sigma) or box (`size` = radius) kernel.
pub fn blur(src: &Image, size: f32, kind: BlurKind) -> Image {
    let kernel = match kind {
        BlurKind::Gaussian => gaussian_kernel(size),
        BlurKind::Box => box_kernel(size.max(0.0).round() as usize),
    };
    convolve_separable(src, &kernel)
}

/// Blur limited by a mask: `lerp(src, blur(src), mask)`.
pub fn blur_masked(src: &Image, size: f32, kind: BlurKind, mask: Option<&Image>) -> Image {
    let blurred = blur(src, size, kind);
    match mask {
        None => blurred,
        Some(m) => {
            let m = m.aligned_to(&src);
            let b = blurred.to_rgba();
            src.map_rgba_xy(|x, y, p| {
                let t = m.get_alpha(x, y);
                let q = b.get_rgba(x, y);
                let mut o = [0.0; 4];
                for i in 0..4 {
                    o[i] = p[i] + (q[i] - p[i]) * t;
                }
                o
            })
        }
    }
}

/// Unsharp mask: `src + amount * (src - gaussian(src, size))`.
pub fn sharpen(src: &Image, amount: f32, size: f32) -> Image {
    let b = blur(src, size, BlurKind::Gaussian);
    let mut out = src.clone();
    out.data
        .par_iter_mut()
        .zip(&b.data)
        .for_each(|(o, bl)| *o += amount * (*o - bl));
    out
}

fn morph_alpha(src: &Image, radius: usize, dilate: bool) -> Image {
    let mut img = src.to_rgba();
    if radius == 0 || img.is_empty() {
        return img;
    }
    let (w, h) = (img.width, img.height);
    let r = radius as i64;
    let pick = |a: f32, b: f32| if dilate { a.max(b) } else { a.min(b) };
    // horizontal pass on alpha
    let alpha: Vec<f32> = img.data.as_chunks::<4>().0.iter().map(|p| p[3]).collect();
    let mut tmp = vec![0.0f32; w * h];
    tmp.par_chunks_exact_mut(w)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let mut v = alpha[y * w + x];
                for dx in -r..=r {
                    let sx = (x as i64 + dx).clamp(0, w as i64 - 1) as usize;
                    v = pick(v, alpha[y * w + sx]);
                }
                *o = v;
            }
        });
    let mut fin = vec![0.0f32; w * h];
    fin.par_chunks_exact_mut(w)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let mut v = tmp[y * w + x];
                for dy in -r..=r {
                    let sy = (y as i64 + dy).clamp(0, h as i64 - 1) as usize;
                    v = pick(v, tmp[sy * w + x]);
                }
                *o = v;
            }
        });
    for (p, a) in img.data.as_chunks_mut::<4>().0.iter_mut().zip(fin) {
        p[3] = a;
    }
    img
}

/// Erode (min-filter) the alpha channel by `radius` pixels.
pub fn erode_alpha(src: &Image, radius: usize) -> Image {
    morph_alpha(src, radius, false)
}

/// Dilate (max-filter) the alpha channel by `radius` pixels.
pub fn dilate_alpha(src: &Image, radius: usize) -> Image {
    morph_alpha(src, radius, true)
}

/// Blur only within the alpha edge band (`dilate(a) - erode(a)`).
pub fn edge_blur(src: &Image, size: f32, edge_width: usize) -> Image {
    let rgba = src.to_rgba();
    let d = dilate_alpha(&rgba, edge_width.max(1));
    let e = erode_alpha(&rgba, edge_width.max(1));
    let mut band = src.blank_like(crate::image::ChannelSet::Alpha);
    band.data
        .par_iter_mut()
        .zip(d.data.par_chunks_exact(4))
        .zip(e.data.par_chunks_exact(4))
        .for_each(|((b, dp), ep)| *b = (dp[3] - ep[3]).clamp(0.0, 1.0));
    blur_masked(&rgba, size, BlurKind::Gaussian, Some(&band))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernels_normalized() {
        let k = gaussian_kernel(2.0);
        assert_eq!(k.len(), 13);
        assert!((k.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!((box_kernel(2).iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn blur_preserves_energy_away_from_edges() {
        let mut img = Image::rgba(64, 64);
        img.set_rgba(32, 32, [1.0, 1.0, 1.0, 1.0]);
        let b = blur(&img, 3.0, BlurKind::Gaussian);
        assert!((b.channel_sum(0) - 1.0).abs() < 1e-4);
        // Gaussian sanity: variance of the resulting distribution is sigma^2.
        let mut var = 0.0f64;
        for x in 0..64 {
            let v: f64 = (0..64).map(|y| b.get(x, y, 0) as f64).sum();
            var += v * ((x as f64) - 32.0).powi(2);
        }
        // (a kernel truncated at 3 sigma loses a little variance)
        assert!((var - 9.0).abs() < 0.25, "variance {var}");
        let bx = blur(&img, 2.0, BlurKind::Box);
        assert!((bx.channel_sum(0) - 1.0).abs() < 1e-4);
        assert!((bx.get(30, 30, 0) - 1.0 / 25.0).abs() < 1e-6);
    }

    #[test]
    fn morphology() {
        let mut img = Image::rgba(9, 9);
        for y in 3..6 {
            for x in 3..6 {
                img.set_rgba(x, y, [1.0; 4]);
            }
        }
        let d = dilate_alpha(&img, 1);
        assert_eq!(d.get_alpha(2, 2), 1.0);
        assert_eq!(d.get_alpha(1, 1), 0.0);
        let e = erode_alpha(&img, 1);
        assert_eq!(e.get_alpha(4, 4), 1.0);
        assert_eq!(e.get_alpha(3, 3), 0.0);
        let eb = edge_blur(&img, 1.0, 1);
        assert_eq!(eb.get_alpha(4, 4), 1.0);
        assert_eq!(eb.get_alpha(0, 0), 0.0);
        assert!(eb.get_alpha(3, 3) < 1.0);
        let s = sharpen(&img, 1.0, 1.0);
        assert!(s.get(4, 4, 0) >= 1.0);
    }
}
