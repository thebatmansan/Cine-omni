//! Keying: chroma (YCbCr distance), luma, difference and screen-subtraction
//! (IBK style) keyers. All produce premultiplied RGBA with despilled color.

use crate::image::Image;

/// Rec.709 luma of linear RGB.
pub fn luma(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Rec.709 Y'CbCr-style decomposition of (linear) RGB; Cb/Cr centered at 0.
pub fn ycbcr(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let y = luma(r, g, b);
    let cb = (b - y) / 1.8556;
    let cr = (r - y) / 1.5748;
    (y, cb, cr)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 <= e0 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Chroma keyer settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChromaKey {
    pub key_color: [f32; 4],
    /// Chroma distance up to which alpha is 0.
    pub tolerance: f32,
    /// Additional distance over which alpha ramps to 1.
    pub softness: f32,
    /// 0..1 amount of spill suppression.
    pub spill_suppress: f32,
    pub invert: bool,
}

/// Suppress spill of the dominant screen channel: the screen channel is pulled
/// down to the mean of the other two where it exceeds them.
pub fn despill(rgb: [f32; 3], key: [f32; 4], amount: f32) -> [f32; 3] {
    let dominant = if key[1] >= key[0] && key[1] >= key[2] {
        1
    } else if key[2] >= key[0] {
        2
    } else {
        0
    };
    let others = match dominant {
        0 => (rgb[1] + rgb[2]) * 0.5,
        1 => (rgb[0] + rgb[2]) * 0.5,
        _ => (rgb[0] + rgb[1]) * 0.5,
    };
    let mut out = rgb;
    if rgb[dominant] > others {
        out[dominant] = rgb[dominant] + (others - rgb[dominant]) * amount.clamp(0.0, 1.0);
    }
    out
}

fn finish(rgb: [f32; 3], alpha: f32, invert: bool) -> [f32; 4] {
    let a = if invert { 1.0 - alpha } else { alpha };
    [rgb[0] * a, rgb[1] * a, rgb[2] * a, a]
}

/// Chroma key: alpha from the CbCr distance to the key color, with spill
/// suppression, output premultiplied.
pub fn chroma_key(src: &Image, k: &ChromaKey) -> Image {
    let (_, kcb, kcr) = ycbcr(k.key_color[0], k.key_color[1], k.key_color[2]);
    src.map_rgba(|p| {
        let (_, cb, cr) = ycbcr(p[0], p[1], p[2]);
        let d = ((cb - kcb).powi(2) + (cr - kcr).powi(2)).sqrt();
        let alpha = smoothstep(k.tolerance, k.tolerance + k.softness, d);
        let rgb = despill([p[0], p[1], p[2]], k.key_color, k.spill_suppress);
        finish(rgb, alpha, k.invert)
    })
}

/// Luma key: alpha ramps from 0 at `low` to 1 at `high`.
pub fn luma_key(src: &Image, low: f32, high: f32, invert: bool) -> Image {
    src.map_rgba(|p| {
        let alpha = smoothstep(low, high, luma(p[0], p[1], p[2]));
        finish([p[0], p[1], p[2]], alpha, invert)
    })
}

/// Difference key against a clean plate: alpha = |src - plate| / threshold.
pub fn difference_key(src: &Image, clean: &Image, threshold: f32, invert: bool) -> Image {
    let clean = clean.to_rgba().aligned_to(&src);
    src.map_rgba_xy(|x, y, p| {
        let c = clean.get_rgba(x, y);
        let d = ((p[0] - c[0]).abs() + (p[1] - c[1]).abs() + (p[2] - c[2]).abs()) / 3.0;
        let alpha = (d / threshold.max(1e-6)).clamp(0.0, 1.0);
        finish([p[0], p[1], p[2]], alpha, invert)
    })
}

/// IBK-style screen subtraction: alpha = 1 - (dominant - max(others)) /
/// (screen dominant - max(screen others)), with despill.
pub fn screen_key(src: &Image, screen: [f32; 4], spill_suppress: f32, invert: bool) -> Image {
    let dominant = if screen[1] >= screen[0] && screen[1] >= screen[2] {
        1
    } else if screen[2] >= screen[0] {
        2
    } else {
        0
    };
    let others_of = |c: [f32; 4]| match dominant {
        0 => c[1].max(c[2]),
        1 => c[0].max(c[2]),
        _ => c[0].max(c[1]),
    };
    let denom = (screen[dominant] - others_of(screen)).max(1e-4);
    src.map_rgba(|p| {
        let excess = (p[dominant] - others_of(p)).max(0.0);
        let alpha = (1.0 - excess / denom).clamp(0.0, 1.0);
        let rgb = despill([p[0], p[1], p[2]], screen, spill_suppress);
        finish(rgb, alpha, invert)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luma_and_difference() {
        let img = Image::ramp(3, 1, true, [0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]);
        let k = luma_key(&img, 0.25, 0.75, false);
        assert_eq!(k.get_alpha(0, 0), 0.0);
        assert_eq!(k.get_alpha(2, 0), 1.0);
        let plate = Image::solid(3, 1, [0.0, 0.0, 0.0, 1.0]);
        let d = difference_key(&img, &plate, 0.5, false);
        assert_eq!(d.get_alpha(0, 0), 0.0);
        assert_eq!(d.get_alpha(2, 0), 1.0);
    }

    #[test]
    fn despill_green() {
        let out = despill([0.2, 0.9, 0.3], [0.0, 1.0, 0.0, 1.0], 1.0);
        assert!((out[1] - 0.25).abs() < 1e-6);
        let none = despill([0.2, 0.9, 0.3], [0.0, 1.0, 0.0, 1.0], 0.0);
        assert_eq!(none, [0.2, 0.9, 0.3]);
    }

    #[test]
    fn screen_key_basics() {
        let img = Image::solid(2, 1, [0.1, 0.9, 0.1, 1.0]);
        let k = screen_key(&img, [0.1, 0.9, 0.1, 1.0], 1.0, false);
        assert!(k.get_alpha(0, 0) < 1e-5);
        let red = Image::solid(1, 1, [1.0, 0.0, 0.0, 1.0]);
        let k = screen_key(&red, [0.1, 0.9, 0.1, 1.0], 1.0, false);
        assert_eq!(k.get_alpha(0, 0), 1.0);
    }
}
