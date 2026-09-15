//! Deep compositing node operations (thin wrappers over [`DeepImage`]).

use crate::error::{Error, Result};
use crate::image::{DeepImage, Image};

/// Merge the sample lists of two deep images.
pub fn deep_merge(a: &DeepImage, b: &DeepImage) -> Result<DeepImage> {
    a.merged(b).ok_or_else(|| {
        Error::SizeMismatch(format!(
            "{}x{} vs {}x{}",
            a.width, a.height, b.width, b.height
        ))
    })
}

/// Flatten a deep image with front-to-back over compositing.
pub fn deep_to_image(d: &DeepImage) -> Image {
    d.flatten()
}

/// Build a deep image from a flat image and a depth pass.
pub fn deep_from_image(image: &Image, depth: &Image) -> Result<DeepImage> {
    let depth = depth.aligned_to(&image);
    DeepImage::from_image(image, &depth)
        .ok_or_else(|| Error::SizeMismatch("deep_from_image".into()))
}

/// Hold out `src` by `holdout`; fully covered samples are removed.
pub fn deep_holdout(src: &DeepImage, holdout: &DeepImage, eps: f32) -> Result<DeepImage> {
    src.holdout(holdout, eps)
        .ok_or_else(|| Error::SizeMismatch("deep_holdout".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::DeepSample;

    #[test]
    fn flatten_matches_manual_over() {
        let mut d = DeepImage::new(1, 1);
        let front = DeepSample::point(1.0, [0.4, 0.0, 0.0, 0.4]);
        let back = DeepSample::point(5.0, [0.0, 0.0, 0.8, 0.8]);
        d.push(0, 0, back);
        d.push(0, 0, front);
        let out = deep_to_image(&d).get_rgba(0, 0);
        let manual = [0.4 + 0.0 * 0.6, 0.0, 0.0 + 0.8 * 0.6, 0.4 + 0.8 * 0.6];
        for i in 0..4 {
            assert!((out[i] - manual[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn holdout_removes_covered() {
        let mut a = DeepImage::new(1, 1);
        a.push(0, 0, DeepSample::point(5.0, [0.0, 0.0, 1.0, 1.0]));
        a.push(0, 0, DeepSample::point(1.0, [1.0, 0.0, 0.0, 1.0]));
        let mut h = DeepImage::new(1, 1);
        h.push(0, 0, DeepSample::point(3.0, [0.0, 0.0, 0.0, 1.0]));
        let out = deep_holdout(&a, &h, 1e-4).unwrap();
        assert_eq!(out.pixel(0, 0).len(), 1);
        assert_eq!(out.pixel(0, 0)[0].z, 1.0);
        let mut half = DeepImage::new(1, 1);
        half.push(0, 0, DeepSample::point(3.0, [0.0, 0.0, 0.0, 0.5]));
        let out = deep_holdout(&a, &half, 1e-4).unwrap();
        assert_eq!(out.pixel(0, 0).len(), 2);
        assert!((out.pixel(0, 0)[1].a - 0.5).abs() < 1e-6);
        let m = deep_merge(&a, &h).unwrap();
        assert_eq!(m.sample_count(), 3);
        assert!(deep_merge(&a, &DeepImage::new(2, 2)).is_err());
    }
}
