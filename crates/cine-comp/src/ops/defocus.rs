//! Depth-driven defocus with a layered circle-of-confusion model.
//!
//! The image is blurred once per slice with linearly increasing sigma
//! (`0 .. max_blur`), and every pixel interpolates between the two slices
//! bracketing its own circle of confusion. Slice 0 is the unblurred input, so
//! pixels exactly on the focal plane are returned unchanged.

use crate::graph_model::BlurKind;
use crate::image::Image;
use crate::ops::filter::blur;
use rayon::prelude::*;

/// Defocus settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZDefocus {
    /// Depth that is in focus.
    pub focal_distance: f32,
    /// Aperture f-number; smaller = more blur.
    pub f_stop: f32,
    /// Maximum blur sigma in pixels.
    pub max_blur: f32,
    /// Number of blur layers (>= 2).
    pub slices: usize,
}

impl Default for ZDefocus {
    fn default() -> Self {
        ZDefocus {
            focal_distance: 10.0,
            f_stop: 4.0,
            max_blur: 8.0,
            slices: 6,
        }
    }
}

/// Aperture scale of the stylised CoC model (`coc = |z-fd|/fd * K / f_stop`).
pub const COC_SCALE: f32 = 16.0;

impl ZDefocus {
    /// Circle of confusion (blur sigma in pixels) for a depth value.
    pub fn coc(&self, z: f32) -> f32 {
        let fd = self.focal_distance.max(1e-4);
        let rel = (z - self.focal_distance).abs() / fd;
        (rel * COC_SCALE / self.f_stop.max(1e-3)).clamp(0.0, self.max_blur.max(0.0))
    }
}

/// Apply depth-of-field blur. `depth` may be any channel set; channel 0 is
/// read as Z (it is cropped/padded to the source size if needed).
pub fn z_defocus(src: &Image, depth: &Image, p: &ZDefocus) -> Image {
    let src = src.to_rgba();
    let depth = if depth.same_size(&src) {
        depth.clone()
    } else {
        depth.crop(0, 0, src.width, src.height)
    };
    let slices = p.slices.max(2);
    if p.max_blur <= 0.0 || src.is_empty() {
        return src;
    }
    let sigmas: Vec<f32> = (0..slices)
        .map(|i| p.max_blur * i as f32 / (slices - 1) as f32)
        .collect();
    let layers: Vec<Image> = sigmas
        .par_iter()
        .map(|&s| {
            if s <= 0.0 {
                src.clone()
            } else {
                blur(&src, s, BlurKind::Gaussian)
            }
        })
        .collect();
    let w = src.width;
    let dnc = depth.nc();
    let mut out = Image::rgba(src.width, src.height);
    out.data
        .par_chunks_exact_mut(4 * w)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let z = depth.data[(y * w + x) * dnc];
                let c = p.coc(z);
                let f = (c / p.max_blur) * (slices - 1) as f32;
                let i0 = (f.floor() as usize).min(slices - 1);
                let i1 = (i0 + 1).min(slices - 1);
                let t = f - i0 as f32;
                let a = layers[i0].get_rgba(x, y);
                if t <= 0.0 || i0 == i1 {
                    px.copy_from_slice(&a);
                } else {
                    let b = layers[i1].get_rgba(x, y);
                    for i in 0..4 {
                        px[i] = a[i] + (b[i] - a[i]) * t;
                    }
                }
            }
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::ChannelSet;

    #[test]
    fn coc_zero_at_focus_and_clamped() {
        let p = ZDefocus::default();
        assert_eq!(p.coc(10.0), 0.0);
        assert!(p.coc(12.0) > 0.0);
        assert_eq!(p.coc(1000.0), p.max_blur);
    }

    #[test]
    fn focus_plane_unchanged_and_far_blurred() {
        let src = Image::checkerboard(32, 32, 2, [1.0; 4], [0.0, 0.0, 0.0, 1.0]);
        let mut depth = Image::new(32, 32, ChannelSet::Depth);
        for y in 0..32 {
            for x in 0..32 {
                depth.set(x, y, 0, if x < 16 { 10.0 } else { 40.0 });
            }
        }
        let out = z_defocus(&src, &depth, &ZDefocus::default());
        assert_eq!(out.get_rgba(4, 4), src.get_rgba(4, 4));
        assert!((out.get_rgba(24, 4)[0] - src.get_rgba(24, 4)[0]).abs() > 0.1);
    }
}
