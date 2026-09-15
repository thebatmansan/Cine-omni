//! Color and channel operations: color correction, premult/unpremult,
//! invert, clamp, channel copy and shuffle, expressions.

use crate::error::{Error, Result};
use crate::expr::{Env, Expr};
use crate::graph_model::{CopyChannels, ShuffleSource};
use crate::image::Image;
use crate::ops::keyer::luma;

/// Color-correct settings. Applied to unpremultiplied RGB in this order:
/// lift → gain → offset → gamma → contrast → saturation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorCorrect {
    pub gain: [f32; 4],
    pub gamma: f32,
    pub lift: [f32; 4],
    pub offset: [f32; 4],
    pub saturation: f32,
    pub contrast: f32,
}

impl Default for ColorCorrect {
    fn default() -> Self {
        ColorCorrect {
            gain: [1.0; 4],
            gamma: 1.0,
            lift: [0.0, 0.0, 0.0, 1.0],
            offset: [0.0, 0.0, 0.0, 1.0],
            saturation: 1.0,
            contrast: 1.0,
        }
    }
}

impl ColorCorrect {
    /// Whether the settings are an identity.
    pub fn is_identity(&self) -> bool {
        *self == ColorCorrect::default()
    }

    /// Apply to one unpremultiplied RGB triple.
    pub fn apply_rgb(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mut out = [0.0f32; 3];
        for i in 0..3 {
            let mut v = rgb[i];
            v = v + self.lift[i] * (1.0 - v);
            v = v * self.gain[i] + self.offset[i];
            if self.gamma != 1.0 && self.gamma > 0.0 {
                v = if v > 0.0 { v.powf(1.0 / self.gamma) } else { v };
            }
            v = (v - 0.18) * self.contrast + 0.18;
            out[i] = v;
        }
        if self.saturation != 1.0 {
            let y = luma(out[0], out[1], out[2]);
            for v in out.iter_mut() {
                *v = y + (*v - y) * self.saturation;
            }
        }
        out
    }
}

/// Unpremultiply an RGBA pixel (alpha 0 leaves RGB untouched).
pub fn unpremult_pixel(p: [f32; 4]) -> [f32; 4] {
    if p[3] > 0.0 {
        [p[0] / p[3], p[1] / p[3], p[2] / p[3], p[3]]
    } else {
        p
    }
}

/// Premultiply an RGBA pixel.
pub fn premult_pixel(p: [f32; 4]) -> [f32; 4] {
    [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]
}

/// Color-correct a premultiplied image, optionally limited by a mask.
pub fn color_correct(src: &Image, cc: &ColorCorrect, mask: Option<&Image>) -> Image {
    let mask = mask.map(|m| m.aligned_to(src));
    src.map_rgba_xy(|x, y, p| {
        let u = unpremult_pixel(p);
        let c = cc.apply_rgb([u[0], u[1], u[2]]);
        // Re-premultiply only where the unpremultiply actually divided.
        // `unpremult_pixel` leaves alpha-zero pixels alone because it cannot
        // divide by zero, so multiplying by that same zero here would not undo
        // a division — it would erase the pixel. Alpha-zero RGB is an additive
        // element (a glow, a flare, fire), and grading one must not destroy it.
        let mut out = if p[3] > 0.0 {
            premult_pixel([c[0], c[1], c[2], p[3]])
        } else {
            [c[0], c[1], c[2], p[3]]
        };
        if let Some(m) = &mask {
            let t = m.get_alpha(x, y);
            for i in 0..4 {
                out[i] = p[i] + (out[i] - p[i]) * t;
            }
        }
        out
    })
}

/// Multiply RGB by alpha.
pub fn premult(src: &Image) -> Image {
    src.map_rgba(premult_pixel)
}

/// Divide RGB by alpha.
pub fn unpremult(src: &Image) -> Image {
    src.map_rgba(unpremult_pixel)
}

/// Invert RGB relative to alpha (premultiplied: `a - c`); alpha untouched.
pub fn invert(src: &Image) -> Image {
    src.map_rgba(|p| [p[3] - p[0], p[3] - p[1], p[3] - p[2], p[3]])
}

/// Clamp every channel to `[min, max]` (alpha optionally).
pub fn clamp(src: &Image, min: f32, max: f32, clamp_alpha: bool) -> Image {
    let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
    src.map_rgba(|p| {
        [
            p[0].clamp(lo, hi),
            p[1].clamp(lo, hi),
            p[2].clamp(lo, hi),
            if clamp_alpha {
                p[3].clamp(lo, hi)
            } else {
                p[3]
            },
        ]
    })
}

/// Copy the selected channels of `b` into `a`.
pub fn copy_channels(a: &Image, b: &Image, which: CopyChannels) -> Image {
    let b = b.to_rgba().aligned_to(a);
    a.map_rgba_xy(|x, y, p| {
        let q = b.get_rgba(x, y);
        match which {
            CopyChannels::Alpha => [p[0], p[1], p[2], q[3]],
            CopyChannels::Rgb => [q[0], q[1], q[2], p[3]],
            CopyChannels::Red => [q[0], p[1], p[2], p[3]],
            CopyChannels::Green => [p[0], q[1], p[2], p[3]],
            CopyChannels::Blue => [p[0], p[1], q[2], p[3]],
            CopyChannels::Rgba => q,
        }
    })
}

/// Rearrange channels: `sources[i]` is what output channel `i` reads.
pub fn shuffle(src: &Image, sources: [ShuffleSource; 4]) -> Image {
    src.map_rgba(|p| {
        let mut out = [0.0; 4];
        for (o, s) in out.iter_mut().zip(sources) {
            *o = match s {
                ShuffleSource::R => p[0],
                ShuffleSource::G => p[1],
                ShuffleSource::B => p[2],
                ShuffleSource::A => p[3],
                ShuffleSource::Zero => 0.0,
                ShuffleSource::One => 1.0,
            };
        }
        out
    })
}

/// Evaluate per-channel expressions (`None` = passthrough) over an image.
pub fn expression(src: &Image, exprs: [Option<&str>; 4]) -> Result<Image> {
    let mut parsed: [Option<Expr>; 4] = [None, None, None, None];
    for (slot, e) in parsed.iter_mut().zip(exprs) {
        if let Some(s) = e {
            if !s.trim().is_empty() {
                *slot = Some(Expr::parse(s).map_err(|e| Error::Expr(format!("`{s}`: {e}")))?);
            }
        }
    }
    let (w, h) = (src.width, src.height);
    Ok(src.map_rgba_xy(|x, y, p| {
        let env = Env::pixel(x, y, w, h, p);
        let mut out = p;
        for (o, e) in out.iter_mut().zip(parsed.iter()) {
            if let Some(e) = e {
                *o = e.eval(&env);
            }
        }
        out
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An additive element carries colour with zero alpha — a glow, a lens
    /// flare, fire. Grading one must not annihilate it.
    #[test]
    fn grading_an_additive_element_preserves_it() {
        let src = Image::solid(2, 2, [1.0, 0.8, 0.6, 0.0]);
        let cc = ColorCorrect {
            gain: [2.0, 2.0, 2.0, 1.0],
            ..Default::default()
        };
        let out = color_correct(&src, &cc, None);
        let p = out.pixel(0, 0);
        assert!(
            p[0] > 0.0,
            "grading an alpha-zero additive element erased it: {p:?}"
        );
        assert!(
            (p[0] - 2.0).abs() < 1e-5,
            "expected gain applied, got {p:?}"
        );
        assert!(
            (p[1] - 1.6).abs() < 1e-5,
            "expected gain applied, got {p:?}"
        );
        assert_eq!(p[3], 0.0, "alpha must stay zero");
    }

    /// The ordinary premultiplied case must be unchanged: grade acts on the
    /// unassociated colour and the result is re-associated.
    #[test]
    fn grading_a_premultiplied_pixel_still_round_trips_through_alpha() {
        let src = Image::solid(2, 2, [0.5, 0.4, 0.3, 0.5]);
        let cc = ColorCorrect {
            gain: [2.0, 2.0, 2.0, 1.0],
            ..Default::default()
        };
        let out = color_correct(&src, &cc, None);
        let p = out.pixel(0, 0);
        // unassociated 1.0 -> gain 2.0 -> re-associated by alpha 0.5 = 1.0
        assert!((p[0] - 1.0).abs() < 1e-5, "got {p:?}");
        assert_eq!(p[3], 0.5);
    }

    #[test]
    fn identity_and_gain() {
        let img = Image::solid(2, 2, [0.25, 0.5, 0.75, 1.0]);
        let same = color_correct(&img, &ColorCorrect::default(), None);
        assert!(same.approx_eq(&img, 1e-6));
        let cc = ColorCorrect {
            gain: [2.0, 2.0, 2.0, 1.0],
            ..Default::default()
        };
        let out = color_correct(&img, &cc, None);
        assert!((out.get_rgba(0, 0)[0] - 0.5).abs() < 1e-6);
        let sat0 = color_correct(
            &img,
            &ColorCorrect {
                saturation: 0.0,
                ..Default::default()
            },
            None,
        );
        let p = sat0.get_rgba(0, 0);
        assert!((p[0] - p[1]).abs() < 1e-6 && (p[1] - p[2]).abs() < 1e-6);
    }

    #[test]
    fn premult_roundtrip_and_shuffle() {
        let img = Image::solid(1, 1, [0.5, 0.25, 0.0, 0.5]);
        let u = unpremult(&img);
        assert_eq!(u.get_rgba(0, 0), [1.0, 0.5, 0.0, 0.5]);
        assert_eq!(premult(&u).get_rgba(0, 0), [0.5, 0.25, 0.0, 0.5]);
        let s = shuffle(
            &img,
            [
                ShuffleSource::A,
                ShuffleSource::R,
                ShuffleSource::Zero,
                ShuffleSource::One,
            ],
        );
        assert_eq!(s.get_rgba(0, 0), [0.5, 0.5, 0.0, 1.0]);
        assert_eq!(invert(&img).get_rgba(0, 0), [0.0, 0.25, 0.5, 0.5]);
        assert_eq!(
            clamp(&Image::solid(1, 1, [2.0, -1.0, 0.5, 3.0]), 0.0, 1.0, true).get_rgba(0, 0),
            [1.0, 0.0, 0.5, 1.0]
        );
        let b = Image::solid(1, 1, [0.1, 0.2, 0.3, 0.9]);
        assert_eq!(
            copy_channels(&img, &b, CopyChannels::Alpha).get_rgba(0, 0),
            [0.5, 0.25, 0.0, 0.9]
        );
    }

    #[test]
    fn expression_node() {
        let img = Image::solid(2, 1, [0.5, 0.25, 0.0, 1.0]);
        let out = expression(&img, [Some("r*0.5+g"), None, Some("x/w"), Some("")]).unwrap();
        assert!((out.get_rgba(0, 0)[0] - 0.5).abs() < 1e-6);
        assert_eq!(out.get_rgba(1, 0)[1], 0.25);
        assert_eq!(out.get_rgba(1, 0)[2], 0.5);
        assert!(expression(&img, [Some("r +"), None, None, None]).is_err());
    }
}
