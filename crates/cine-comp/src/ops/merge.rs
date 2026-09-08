//! Porter-Duff and arithmetic merge operations on premultiplied RGBA.

use crate::graph_model::MergeOp;
use crate::image::Image;

/// Merge one premultiplied pixel `a` (foreground) with `b` (background).
pub fn merge_pixel(op: MergeOp, a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let aa = a[3];
    let ba = b[3];
    let mut out = [0.0f32; 4];
    for i in 0..4 {
        let x = a[i];
        let y = b[i];
        out[i] = match op {
            MergeOp::Over => x + y * (1.0 - aa),
            MergeOp::Under => x * (1.0 - ba) + y,
            MergeOp::Plus => x + y,
            MergeOp::Minus => x - y,
            MergeOp::Multiply => {
                if i == 3 {
                    aa + ba - aa * ba
                } else {
                    x * y
                }
            }
            MergeOp::Screen => x + y - x * y,
            MergeOp::Difference => (x - y).abs(),
            MergeOp::Max => x.max(y),
            MergeOp::Min => x.min(y),
            MergeOp::Mask => y * aa,
            MergeOp::Stencil => y * (1.0 - aa),
            MergeOp::Xor => x * (1.0 - ba) + y * (1.0 - aa),
            MergeOp::In => x * ba,
            MergeOp::Out => x * (1.0 - ba),
            MergeOp::Atop => x * ba + y * (1.0 - aa),
        };
    }
    out
}

/// Merge image `a` onto `b`. `b` defines the output format; `a` is
/// cropped/padded (with transparent black) to match when the sizes differ.
/// `mask` (alpha promoted) and `mix` blend the result back towards `b`.
pub fn merge(op: MergeOp, a: &Image, b: &Image, mask: Option<&Image>, mix: f32) -> Image {
    let a = if a.same_size(b) {
        a.to_rgba()
    } else {
        a.to_rgba().crop(0, 0, b.width, b.height)
    };
    let mask = mask.map(|m| {
        if m.same_size(b) {
            m.clone()
        } else {
            m.crop(0, 0, b.width, b.height)
        }
    });
    b.map_rgba_xy(|x, y, bp| {
        let ap = a.get_rgba(x, y);
        let r = merge_pixel(op, ap, bp);
        let m = mix * mask.as_ref().map(|m| m.get_alpha(x, y)).unwrap_or(1.0);
        if m >= 1.0 {
            r
        } else {
            let mut o = [0.0; 4];
            for i in 0..4 {
                o[i] = bp[i] + (r[i] - bp[i]) * m;
            }
            o
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_premultiplied() {
        // 50% alpha red (premultiplied) over opaque blue.
        let a = [0.5, 0.0, 0.0, 0.5];
        let b = [0.0, 0.0, 1.0, 1.0];
        assert_eq!(merge_pixel(MergeOp::Over, a, b), [0.5, 0.0, 0.5, 1.0]);
        assert_eq!(merge_pixel(MergeOp::Under, b, a), [0.5, 0.0, 0.5, 1.0]);
    }

    #[test]
    fn arithmetic_ops() {
        let a = [0.5, 0.25, 0.0, 0.5];
        let b = [0.25, 0.25, 1.0, 1.0];
        assert_eq!(merge_pixel(MergeOp::Plus, a, b), [0.75, 0.5, 1.0, 1.5]);
        assert_eq!(merge_pixel(MergeOp::Minus, a, b), [0.25, 0.0, -1.0, -0.5]);
        assert_eq!(
            merge_pixel(MergeOp::Multiply, a, b),
            [0.125, 0.0625, 0.0, 1.0]
        );
        assert_eq!(merge_pixel(MergeOp::Mask, a, b), [0.125, 0.125, 0.5, 0.5]);
        assert_eq!(
            merge_pixel(MergeOp::Stencil, a, b),
            [0.125, 0.125, 0.5, 0.5]
        );
        assert_eq!(merge_pixel(MergeOp::Max, a, b), [0.5, 0.25, 1.0, 1.0]);
        assert_eq!(merge_pixel(MergeOp::Min, a, b), [0.25, 0.25, 0.0, 0.5]);
        assert_eq!(
            merge_pixel(MergeOp::Difference, a, b),
            [0.25, 0.0, 1.0, 0.5]
        );
        assert_eq!(merge_pixel(MergeOp::In, a, b), a);
        assert_eq!(merge_pixel(MergeOp::Out, a, b), [0.0; 4]);
        assert_eq!(merge_pixel(MergeOp::Screen, [0.5; 4], [0.5; 4]), [0.75; 4]);
    }

    #[test]
    fn mix_and_mask() {
        let a = Image::solid(2, 1, [1.0, 0.0, 0.0, 1.0]);
        let b = Image::solid(2, 1, [0.0, 0.0, 1.0, 1.0]);
        let half = merge(MergeOp::Over, &a, &b, None, 0.5);
        assert_eq!(half.get_rgba(0, 0), [0.5, 0.0, 0.5, 1.0]);
        let mut mask = Image::new(2, 1, crate::image::ChannelSet::Alpha);
        mask.set(1, 0, 0, 1.0);
        let masked = merge(MergeOp::Over, &a, &b, Some(&mask), 1.0);
        assert_eq!(masked.get_rgba(0, 0), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(masked.get_rgba(1, 0), [1.0, 0.0, 0.0, 1.0]);
    }
}
