//! Geometric operations: affine transform, crop, reformat.

use crate::error::{Error, Result};
use crate::graph_model::FilterKind;
use crate::image::{Image, RegionOfInterest};
use glam::{Affine2, Vec2};
use rayon::prelude::*;

/// The largest result a transform will produce: 32 million pixels, half a
/// gigabyte of RGBA float. A transform that would exceed it is refused with
/// its numbers rather than allocating until the process dies, and rather than
/// quietly cropping to something that fits — which is what losing the moved
/// pixels in the first place looked like.
pub const MAX_TRANSFORM_PIXELS: usize = 32 << 20;

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
        self.matrix_on_canvas(0, 0, width, height)
    }

    /// Forward matrix in canvas coordinates, for an image whose data window
    /// starts at `(x, y)`. The default pivot is the centre of that window, so
    /// an image and a crop of it turn about the same point.
    pub fn matrix_on_canvas(&self, x: i64, y: i64, width: usize, height: usize) -> Affine2 {
        let c = self.center.map(Vec2::from).unwrap_or(Vec2::new(
            x as f32 + (width as f32 - 1.0) / 2.0,
            y as f32 + (height as f32 - 1.0) / 2.0,
        ));
        Affine2::from_translation(Vec2::from(self.translate) + c)
            * Affine2::from_angle(self.rotate_deg.to_radians())
            * Affine2::from_scale(Vec2::new(self.scale[0].max(1e-6), self.scale[1].max(1e-6)))
            * Affine2::from_translation(-c)
    }

    /// How far a sample reaches around its centre, in pixels, for a transform
    /// that lands sample points between source pixels.
    fn filter_support(&self) -> f32 {
        match self.filter {
            FilterKind::Bilinear => 1.0,
            FilterKind::Bicubic => 2.0,
        }
    }
}

/// The data window a transform produces from a source window: the transformed
/// corners, grown by the filter's reach.
///
/// This is the whole of the fix the audit asked for. A transform used to write
/// into a buffer the size of its source, so translating a plate 500 pixels
/// right threw away 500 pixels of it — the pixels the artist was moving.
fn transformed_window(src: &Image, m: &Affine2, support: f32) -> (i64, i64, usize, usize) {
    let (x, y, w, h) = src.window();
    // Corners through the outermost pixel *centres*; the filter's reach is
    // added afterwards, and only when sampling actually falls between pixels.
    let (x1, y1) = (
        (x + w as i64 - 1).max(x) as f32,
        (y + h as i64 - 1).max(y) as f32,
    );
    let corners = [
        Vec2::new(x as f32, y as f32),
        Vec2::new(x1, y as f32),
        Vec2::new(x as f32, y1),
        Vec2::new(x1, y1),
    ];
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    for c in corners {
        let p = m.transform_point2(c);
        min = min.min(p);
        max = max.max(p);
    }
    let x0 = (min.x - support).floor() as i64;
    let y0 = (min.y - support).floor() as i64;
    let x1 = (max.x + support).ceil() as i64;
    let y1 = (max.y + support).ceil() as i64;
    (
        x0,
        y0,
        (x1 - x0 + 1).max(0) as usize,
        (y1 - y0 + 1).max(0) as usize,
    )
}

/// Whether `m` lands every source pixel centre exactly on a destination pixel
/// centre. Then no filter runs, and the result needs no margin: an integer
/// translate stays exact, and its window is the source's window moved.
fn lands_on_pixels(m: &Affine2) -> bool {
    let near = |a: f32, b: f32| (a - b).abs() < 1e-6;
    let integral = |v: f32| (v - v.round()).abs() < 1e-6;
    near(m.matrix2.x_axis.x, 1.0)
        && near(m.matrix2.x_axis.y, 0.0)
        && near(m.matrix2.y_axis.x, 0.0)
        && near(m.matrix2.y_axis.y, 1.0)
        && integral(m.translation.x)
        && integral(m.translation.y)
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

/// The source region a transform must read to produce `roi`.
///
/// The region mapped back through the transform, grown by the filter's reach.
/// Used to pull only what is needed up the chain; the pivot must be explicit,
/// since the automatic one depends on the source that has not been read yet.
pub fn source_region_for(t: &Transform2D, roi: RegionOfInterest) -> RegionOfInterest {
    let m = t.matrix_on_canvas(0, 0, 0, 0);
    let inv = m.inverse();
    let (x0, y0) = (roi.x as f32, roi.y as f32);
    let (x1, y1) = ((roi.x + roi.width) as f32, (roi.y + roi.height) as f32);
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    for c in [
        Vec2::new(x0, y0),
        Vec2::new(x1, y0),
        Vec2::new(x0, y1),
        Vec2::new(x1, y1),
    ] {
        let p = inv.transform_point2(c);
        min = min.min(p);
        max = max.max(p);
    }
    let support = t.filter_support();
    let sx = (min.x - support).floor().max(0.0) as usize;
    let sy = (min.y - support).floor().max(0.0) as usize;
    let ex = (max.x + support).ceil().max(0.0) as usize;
    let ey = (max.y + support).ceil().max(0.0) as usize;
    RegionOfInterest::new(sx, sy, ex.saturating_sub(sx) + 1, ey.saturating_sub(sy) + 1)
}

/// Apply an affine transform. The result's data window is the source's window
/// transformed, so nothing that was moved is lost; uncovered pixels inside it
/// are transparent black.
pub fn transform(src: &Image, t: &Transform2D) -> Result<Image> {
    transform_roi(src, t, None)
}

/// A transform computing only `roi`, when one is asked for.
///
/// Only the pixels of the region are produced, and only the part of the
/// transformed window that meets it: a viewer asking for a corner of a
/// transformed 4K plate pays for the corner.
pub fn transform_roi(src: &Image, t: &Transform2D, roi: Option<RegionOfInterest>) -> Result<Image> {
    let src = src.to_rgba();
    let (sx, sy, _, _) = src.window();
    let m = t.matrix_on_canvas(sx, sy, src.width, src.height);
    let inv = m.inverse();
    let support = if lands_on_pixels(&m) {
        0.0
    } else {
        t.filter_support()
    };
    let (mut x0, mut y0, mut w, mut h) = transformed_window(&src, &m, support);
    if let Some(r) = roi {
        let (rx0, ry0) = (r.x as i64, r.y as i64);
        let (rx1, ry1) = (rx0 + r.width as i64, ry0 + r.height as i64);
        let (nx0, ny0) = (x0.max(rx0), y0.max(ry0));
        let (nx1, ny1) = ((x0 + w as i64).min(rx1), (y0 + h as i64).min(ry1));
        x0 = nx0;
        y0 = ny0;
        w = (nx1 - nx0).max(0) as usize;
        h = (ny1 - ny0).max(0) as usize;
    }
    if src.is_empty() || w == 0 || h == 0 {
        return Ok(Image::rgba(0, 0).placed(x0, y0));
    }
    if w.saturating_mul(h) > MAX_TRANSFORM_PIXELS {
        return Err(Error::Refused(format!(
            "this transform would produce {w}×{h} = {} pixels ({:.1} GB of RGBA float); the \
             limit is {MAX_TRANSFORM_PIXELS} pixels. Crop or reformat first, or ask for a region",
            w * h,
            (w as f64 * h as f64 * 16.0) / 1e9,
        )));
    }
    let mut out = Image::rgba(w, h).placed(x0, y0);
    out.data
        .par_chunks_exact_mut(4 * w)
        .enumerate()
        .for_each(|(row_y, row)| {
            let cy = (y0 + row_y as i64) as f32;
            for (row_x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let p = inv.transform_point2(Vec2::new((x0 + row_x as i64) as f32, cy));
                // Source-local coordinates: the sampler works on the buffer.
                let (lx, ly) = (p.x - sx as f32, p.y - sy as f32);
                if lx <= -1.0 || ly <= -1.0 || lx >= src.width as f32 || ly >= src.height as f32 {
                    continue;
                }
                let s = match t.filter {
                    FilterKind::Bilinear => sample_bilinear_zero(&src, lx, ly),
                    FilterKind::Bicubic => sample_bicubic_zero(&src, lx, ly),
                };
                px.copy_from_slice(&s);
            }
        });
    Ok(out)
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

    /// A translate moves the picture; it does not cut a hole in it. The
    /// window moves with the pixels, which is the whole point of a data
    /// window, and every pixel survives.
    #[test]
    fn integer_translate_moves_the_window_and_loses_nothing() {
        let img = Image::checkerboard(16, 16, 3, [1.0, 0.2, 0.3, 1.0], [0.1, 0.9, 0.5, 1.0]);
        let t = Transform2D {
            translate: [3.0, -2.0],
            ..Default::default()
        };
        let out = transform(&img, &t).unwrap();
        assert_eq!(
            out.window(),
            (3, -2, 16, 16),
            "the window moved with the pixels"
        );
        for y in 0..16 {
            for x in 0..16 {
                assert_eq!(
                    out.canvas_rgba(x as i64 + 3, y as i64 - 2),
                    img.get_rgba(x, y),
                    "pixel {x},{y}"
                );
            }
        }
        assert_eq!(
            out.canvas_rgba(0, 0),
            [0.0; 4],
            "nothing outside the window"
        );
    }

    /// A big translate used to throw the picture away entirely: the output was
    /// the source's window, and the pixels had all moved out of it.
    #[test]
    fn a_translate_past_the_source_window_still_has_the_picture() {
        let mut img = Image::rgba(8, 8);
        img.set_rgba(2, 2, [1.0, 0.5, 0.25, 1.0]);
        let out = transform(
            &img,
            &Transform2D {
                translate: [500.0, 300.0],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(out.window(), (500, 300, 8, 8));
        assert_eq!(out.canvas_rgba(502, 302), [1.0, 0.5, 0.25, 1.0]);
    }

    #[test]
    fn rotate_and_scale_keep_center_and_grow_the_window() {
        let mut img = Image::rgba(9, 9);
        img.set_rgba(4, 4, [1.0; 4]);
        let t = Transform2D {
            rotate_deg: 90.0,
            scale: [2.0, 2.0],
            ..Default::default()
        };
        let out = transform(&img, &t).unwrap();
        assert!(
            (out.canvas_rgba(4, 4)[0] - 1.0).abs() < 1e-5,
            "the pivot is fixed"
        );
        let (x, y, w, h) = out.window();
        assert!(
            x < 0 && y < 0 && w >= 18 && h >= 18,
            "a 2x scale needs a bigger window: {:?}",
            out.window()
        );
        let bic = transform(
            &img,
            &Transform2D {
                filter: FilterKind::Bicubic,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            bic.approx_eq(&img, 1e-5),
            "the identity is still the identity"
        );
    }

    /// A region asked for is a region produced: the rest is not computed.
    #[test]
    fn a_region_bounds_what_a_transform_produces() {
        let img = Image::checkerboard(64, 64, 4, [1.0; 4], [0.0, 0.0, 0.0, 1.0]);
        let t = Transform2D {
            translate: [100.0, 100.0],
            scale: [4.0, 4.0],
            ..Default::default()
        };
        let whole = transform(&img, &t).unwrap();
        let roi = RegionOfInterest::new(120, 120, 16, 16);
        let part = transform_roi(&img, &t, Some(roi)).unwrap();
        assert_eq!(part.window(), (120, 120, 16, 16));
        assert!(
            part.data.len() * 20 < whole.data.len(),
            "the region is a fraction of the frame"
        );
        for y in 120..136 {
            for x in 120..136 {
                assert_eq!(part.canvas_rgba(x, y), whole.canvas_rgba(x, y), "{x},{y}");
            }
        }
    }

    /// A transform that would not fit in memory is refused with its numbers,
    /// not attempted and not quietly cropped.
    #[test]
    fn an_impossible_transform_is_refused_with_its_numbers() {
        let img = Image::rgba(4096, 4096);
        let out = transform(
            &img,
            &Transform2D {
                scale: [40.0, 40.0],
                ..Default::default()
            },
        );
        let Err(e) = out else {
            panic!("a 163840² transform must not be attempted");
        };
        let why = e.to_string();
        assert!(
            why.contains("163804×163804"),
            "the size it would need: {why}"
        );
        assert!(why.contains("GB"), "what that costs: {why}");
        assert!(why.contains("limit is"), "and what the limit is: {why}");
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
