//! Geometric operations: affine transform, crop, reformat.

use crate::graph_model::FilterKind;
use crate::image::Image;
use glam::{Affine2, Vec2};
use rayon::prelude::*;

/// Affine transform settings (pixels; rotation clockwise in image space).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform2D {
    pub translate: [f32; 2],
    pub rotate_deg: f32,
    pub scale: [f32; 2],
    /// Pivot; `None` uses the image center `((w-1)/2, (h-1)/2)` (integer
    /// coordinates are pixel centers).
    pub center: Option<[f32; 2]>,
    pub filter: FilterKind,
}

impl Default for Transform2D {
    fn default() -> Self {
        Transform2D {
            translate: [0.0, 0.0],
            rotate_deg: 0.0,
            scale: [1.0, 1.0],
            center: None,
            filter: FilterKind::Bilinear,
        }
    }
}

impl Transform2D {
    /// Forward matrix mapping source pixel coordinates to destination.
    pub fn matrix(&self, width: usize, height: usize) -> Affine2 {
        let c = self.center.map(Vec2::from).unwrap_or(Vec2::new(
            (width as f32 - 1.0) / 2.0,
            (height as f32 - 1.0) / 2.0,
        ));
        Affine2::from_translation(Vec2::from(self.translate) + c)
            * Affine2::from_angle(self.rotate_deg.to_radians())
            * Affine2::from_scale(Vec2::new(self.scale[0].max(1e-6), self.scale[1].max(1e-6)))
            * Affine2::from_translation(-c)
    }
}

/// RGBA at integer coordinates, transparent black outside the image.
fn fetch(img: &Image, x: i64, y: i64) -> [f32; 4] {
    if img.contains(x, y) {
        img.get_rgba(x as usize, y as usize)
    } else {
        [0.0; 4]
    }
}

/// Bilinear sample with transparent black outside the image.
pub fn sample_bilinear_zero(img: &Image, x: f32, y: f32) -> [f32; 4] {
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let (xi, yi) = (x0 as i64, y0 as i64);
    let p00 = fetch(img, xi, yi);
    let p10 = fetch(img, xi + 1, yi);
    let p01 = fetch(img, xi, yi + 1);
    let p11 = fetch(img, xi + 1, yi + 1);
    let mut out = [0.0; 4];
    for i in 0..4 {
        let top = p00[i] + (p10[i] - p00[i]) * fx;
        let bot = p01[i] + (p11[i] - p01[i]) * fx;
        out[i] = top + (bot - top) * fy;
    }
    out
}

/// Catmull-Rom bicubic sample with transparent black outside the image.
pub fn sample_bicubic_zero(img: &Image, x: f32, y: f32) -> [f32; 4] {
    fn w(t: f32) -> [f32; 4] {
        let t2 = t * t;
        let t3 = t2 * t;
        [
            -0.5 * t3 + t2 - 0.5 * t,
            1.5 * t3 - 2.5 * t2 + 1.0,
            -1.5 * t3 + 2.0 * t2 + 0.5 * t,
            0.5 * t3 - 0.5 * t2,
        ]
    }
    let x0 = x.floor();
    let y0 = y.floor();
    let wx = w(x - x0);
    let wy = w(y - y0);
    let mut acc = [0.0f32; 4];
    for (j, wyj) in wy.iter().enumerate() {
        for (i, wxi) in wx.iter().enumerate() {
            let p = fetch(img, x0 as i64 + i as i64 - 1, y0 as i64 + j as i64 - 1);
            let wt = wxi * wyj;
            for (a, v) in acc.iter_mut().zip(p) {
                *a += v * wt;
            }
        }
    }
    acc
}

/// Apply an affine transform; the output keeps the source format and
/// uncovered pixels are transparent black.
pub fn transform(src: &Image, t: &Transform2D) -> Image {
    let src = src.to_rgba();
    let m = t.matrix(src.width, src.height);
    let inv = m.inverse();
    let w = src.width;
    let mut out = src.blank_like(crate::image::ChannelSet::Rgba);
    if src.is_empty() {
        return out;
    }
    out.data
        .par_chunks_exact_mut(4 * w)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let p = inv.transform_point2(Vec2::new(x as f32, y as f32));
                if p.x <= -1.0 || p.y <= -1.0 || p.x >= src.width as f32 || p.y >= src.height as f32
                {
                    continue;
                }
                let s = match t.filter {
                    FilterKind::Bilinear => sample_bilinear_zero(&src, p.x, p.y),
                    FilterKind::Bicubic => sample_bicubic_zero(&src, p.x, p.y),
                };
                px.copy_from_slice(&s);
            }
        });
    out
}

/// Crop the canvas rectangle at `(x, y)` into a new `w × h` format whose
/// origin is its top-left (pixels outside the source's data window are
/// transparent black).
pub fn crop(src: &Image, x: i64, y: i64, w: usize, h: usize) -> Image {
    src.crop_canvas(x, y, w, h).placed(0, 0)
}

/// Resize to a new format.
pub fn reformat(src: &Image, w: usize, h: usize, filter: FilterKind) -> Image {
    match filter {
        FilterKind::Bilinear => src.resize(w, h),
        FilterKind::Bicubic => {
            let src = src.to_rgba();
            let mut out = Image::rgba(w, h);
            if src.is_empty() || w == 0 || h == 0 {
                return out;
            }
            let sx = src.width as f32 / w as f32;
            let sy = src.height as f32 / h as f32;
            out.data
                .par_chunks_exact_mut(4 * w)
                .enumerate()
                .for_each(|(y, row)| {
                    let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
                    for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
                        px.copy_from_slice(&src.sample_bicubic(fx, fy));
                    }
                });
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_translate_is_exact() {
        let img = Image::checkerboard(16, 16, 3, [1.0, 0.2, 0.3, 1.0], [0.1, 0.9, 0.5, 1.0]);
        let t = Transform2D {
            translate: [3.0, -2.0],
            ..Default::default()
        };
        let out = transform(&img, &t);
        for y in 0..14 {
            for x in 3..16 {
                assert_eq!(out.get_rgba(x, y), img.get_rgba(x - 3, y + 2));
            }
        }
        assert_eq!(out.get_rgba(0, 0), [0.0; 4]);
        assert_eq!(out.get_rgba(5, 15), [0.0; 4]);
    }

    #[test]
    fn rotate_and_scale_keep_center() {
        let mut img = Image::rgba(9, 9);
        img.set_rgba(4, 4, [1.0; 4]);
        let t = Transform2D {
            rotate_deg: 90.0,
            scale: [2.0, 2.0],
            ..Default::default()
        };
        let out = transform(&img, &t);
        assert!((out.get_rgba(4, 4)[0] - 1.0).abs() < 1e-5);
        let bic = transform(
            &img,
            &Transform2D {
                filter: FilterKind::Bicubic,
                ..Default::default()
            },
        );
        assert!(bic.approx_eq(&img, 1e-5));
    }

    #[test]
    fn reformat_sizes() {
        let img = Image::ramp(8, 8, true, [0.0; 4], [1.0; 4]);
        let a = reformat(&img, 4, 2, FilterKind::Bilinear);
        let b = reformat(&img, 4, 2, FilterKind::Bicubic);
        assert_eq!((a.width, a.height), (4, 2));
        assert_eq!((b.width, b.height), (4, 2));
        assert!(a.max_abs_diff(&b) < 0.1);
    }
}
