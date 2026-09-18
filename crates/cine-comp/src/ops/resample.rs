//! Changing a picture's size, and fitting one picture into another's frame.
//!
//! ## The filter
//!
//! Resizing is separable Lanczos-3. Shrinking widens the kernel by the ratio,
//! so every source pixel contributes and fine detail averages rather than
//! aliasing. Weights for each output
//! row and column are normalised to sum to one, so flat areas stay exactly
//! flat at any ratio. Lanczos rings slightly at hard edges; values are not
//! clamped here, because clamping is a decision the encoding makes. At the
//! picture's border the kernel is cut off and renormalised, so the outermost
//! few output pixels of a busy edge are slightly biased (measured: 0.04 of
//! full scale for one-pixel stripes at 4:1; exact from the fourth pixel in).
//!
//! Filtering happens in whatever encoding the pixels are in. For display-
//! encoded video, which is what a render scales, that is the convention of
//! video scalers; it is not linear light, and nothing here claims it is.
//!
//! ## Fitting
//!
//! A picture of one aspect ratio placed in a frame of another can be
//! letterboxed (all of it shown, bars added), cropped (the frame filled, edges
//! lost) or stretched (distorted). Each is a different picture. [`Fit::Refuse`]
//! is the default because none of them should happen by accident, and
//! [`fit`] reports what it did — the scale, the region of the output the picture
//! covers, and the region of the source that was kept — so the choice can be
//! recorded with the output.

use crate::error::{Error, Result};
use crate::image::{ChannelSet, Image};
use serde::Serialize;

/// Lobes of the Lanczos window.
const LOBES: f64 = 3.0;

fn lanczos(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    if x.abs() >= LOBES {
        return 0.0;
    }
    let px = std::f64::consts::PI * x;
    LOBES * px.sin() * (px / LOBES).sin() / (px * px)
}

/// For each output sample, the first source index and its normalised weights.
fn weights(src: usize, dst: usize, offset: f64, span: f64) -> Vec<(usize, Vec<f32>)> {
    // `span` source samples map onto `dst` output samples, starting `offset`
    // source samples in.
    let scale = span / dst as f64;
    let support = LOBES * scale.max(1.0);
    let stretch = scale.max(1.0);
    (0..dst)
        .map(|i| {
            let centre = offset + (i as f64 + 0.5) * scale - 0.5;
            let lo = (centre - support).floor().max(0.0) as usize;
            let hi = ((centre + support).ceil() as usize).min(src.saturating_sub(1));
            let mut w: Vec<f64> = (lo..=hi)
                .map(|j| lanczos((j as f64 - centre) / stretch))
                .collect();
            let sum: f64 = w.iter().sum();
            if sum.abs() > 1e-12 {
                for v in &mut w {
                    *v /= sum;
                }
            }
            (lo, w.into_iter().map(|v| v as f32).collect())
        })
        .collect()
}

/// Resize the region `(x, y, w, h)` of `src`, in source pixels (fractional
/// allowed), to `out_w × out_h`.
pub fn resize_region(
    src: &Image,
    region: (f64, f64, f64, f64),
    out_w: usize,
    out_h: usize,
) -> Result<Image> {
    if src.is_empty() || out_w == 0 || out_h == 0 {
        return Err(Error::Refused(format!(
            "cannot resize {}x{} to {out_w}x{out_h}",
            src.width, src.height
        )));
    }
    let nc = src.nc();
    let (rx, ry, rw, rh) = region;
    let cols = weights(src.width, out_w, rx, rw);
    let rows = weights(src.height, out_h, ry, rh);
    // Horizontal pass: src.height rows of out_w. The accumulator is a fixed
    // four-channel array for the usual images, which lets the inner loop run
    // without bounds checks on every sample; anything wider takes the general
    // path.
    let mut mid = vec![0.0f32; src.height * out_w * nc];
    for y in 0..src.height {
        let line = &src.data[y * src.width * nc..(y + 1) * src.width * nc];
        for (x, (lo, w)) in cols.iter().enumerate() {
            let o = (y * out_w + x) * nc;
            let taps = &line[lo * nc..(lo + w.len()) * nc];
            if nc <= 4 {
                let mut acc = [0.0f32; 4];
                for (px, wk) in taps.chunks_exact(nc).zip(w) {
                    for (a, v) in acc.iter_mut().zip(px) {
                        *a += wk * v;
                    }
                }
                mid[o..o + nc].copy_from_slice(&acc[..nc]);
            } else {
                for (px, wk) in taps.chunks_exact(nc).zip(w) {
                    for (m, v) in mid[o..o + nc].iter_mut().zip(px) {
                        *m += wk * v;
                    }
                }
            }
        }
    }
    // Vertical pass: whole rows, which are contiguous.
    let stride = out_w * nc;
    let mut data = vec![0.0f32; out_h * stride];
    for (y, (lo, w)) in rows.iter().enumerate() {
        let out = &mut data[y * stride..(y + 1) * stride];
        for (k, wk) in w.iter().enumerate() {
            let row = &mid[(lo + k) * stride..(lo + k + 1) * stride];
            for (o, v) in out.iter_mut().zip(row) {
                *o += wk * v;
            }
        }
    }
    Image::from_data(out_w, out_h, src.channels.clone(), data)
        .ok_or_else(|| Error::Refused("resize produced the wrong buffer size".into()))
}

/// Resize all of `src` to `out_w × out_h`.
pub fn resize(src: &Image, out_w: usize, out_h: usize) -> Result<Image> {
    resize_region(
        src,
        (0.0, 0.0, src.width as f64, src.height as f64),
        out_w,
        out_h,
    )
}

/// How a picture is placed in a frame of a different size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    /// Different aspect ratios are an error. The same aspect ratio at a
    /// different size is scaled, which distorts nothing.
    Refuse,
    /// Scale to fit inside; bars of `background` fill the rest.
    Letterbox,
    /// Scale to cover; the picture's overflowing edges are cut, centred.
    Crop,
    /// Scale each axis independently to the frame. Distorts.
    Stretch,
}

impl Fit {
    pub fn parse(s: &str) -> std::result::Result<Fit, String> {
        match s {
            "refuse" => Ok(Fit::Refuse),
            "letterbox" => Ok(Fit::Letterbox),
            "crop" => Ok(Fit::Crop),
            "stretch" => Ok(Fit::Stretch),
            other => Err(format!(
                "unknown fit `{other}` (known: refuse, letterbox, crop, stretch)"
            )),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Fit::Refuse => "refuse",
            Fit::Letterbox => "letterbox",
            Fit::Crop => "crop",
            Fit::Stretch => "stretch",
        }
    }
}

/// What fitting did, for the record.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Placement {
    /// Output pixels per source pixel, horizontally and vertically.
    pub scale: (f64, f64),
    /// The part of the output the picture covers: x, y, width, height.
    pub covers: (usize, usize, usize, usize),
    /// The part of the source that is shown: x, y, width, height.
    pub keeps: (f64, f64, f64, f64),
}

/// Work out where a `src_w × src_h` picture goes in an `out_w × out_h` frame.
///
/// Aspect ratios are compared exactly, as integers: `1920×1080` and `1280×720`
/// are the same shape, `720×576` and `1024×576` are not.
pub fn placement(
    src_w: usize,
    src_h: usize,
    out_w: usize,
    out_h: usize,
    fit: Fit,
) -> std::result::Result<Placement, String> {
    if src_w == 0 || src_h == 0 || out_w == 0 || out_h == 0 {
        return Err("a picture or frame of zero size".into());
    }
    let same_shape = (src_w as u128) * (out_h as u128) == (src_h as u128) * (out_w as u128);
    let whole = (0.0, 0.0, src_w as f64, src_h as f64);
    if same_shape || fit == Fit::Stretch {
        return Ok(Placement {
            scale: (out_w as f64 / src_w as f64, out_h as f64 / src_h as f64),
            covers: (0, 0, out_w, out_h),
            keeps: whole,
        });
    }
    let sx = out_w as f64 / src_w as f64;
    let sy = out_h as f64 / src_h as f64;
    match fit {
        Fit::Refuse => Err(format!(
            "a {src_w}x{src_h} picture is a different shape from a {out_w}x{out_h} frame; \
             choose letterbox, crop or stretch"
        )),
        Fit::Letterbox => {
            let s = sx.min(sy);
            // Whole pixels, centred; the picture never overflows the frame.
            let w = ((src_w as f64 * s).round() as usize).clamp(1, out_w);
            let h = ((src_h as f64 * s).round() as usize).clamp(1, out_h);
            Ok(Placement {
                scale: (w as f64 / src_w as f64, h as f64 / src_h as f64),
                covers: ((out_w - w) / 2, (out_h - h) / 2, w, h),
                keeps: whole,
            })
        }
        Fit::Crop => {
            let s = sx.max(sy);
            let kw = (out_w as f64 / s).min(src_w as f64);
            let kh = (out_h as f64 / s).min(src_h as f64);
            Ok(Placement {
                scale: (s, s),
                covers: (0, 0, out_w, out_h),
                keeps: ((src_w as f64 - kw) / 2.0, (src_h as f64 - kh) / 2.0, kw, kh),
            })
        }
        Fit::Stretch => unreachable!("handled above"),
    }
}

/// Place `src` into an `out_w × out_h` RGBA frame of `background` as `p` says.
pub fn fit(
    src: &Image,
    out_w: usize,
    out_h: usize,
    p: &Placement,
    background: [f32; 4],
) -> Result<Image> {
    let src = if src.channels == ChannelSet::Rgba {
        std::borrow::Cow::Borrowed(src)
    } else {
        std::borrow::Cow::Owned(src.to_rgba())
    };
    let (cx, cy, cw, ch) = p.covers;
    if cx == 0
        && cy == 0
        && cw == out_w
        && ch == out_h
        && cw == src.width
        && ch == src.height
        && p.keeps == (0.0, 0.0, src.width as f64, src.height as f64)
    {
        return Ok(src.into_owned());
    }
    let scaled = resize_region(&src, p.keeps, cw, ch)?;
    if (cx, cy, cw, ch) == (0, 0, out_w, out_h) {
        return Ok(scaled);
    }
    let mut out = Image::rgba(out_w, out_h);
    for px in out.data.as_chunks_mut::<4>().0 {
        px.copy_from_slice(&background);
    }
    for y in 0..ch {
        let s = y * cw * 4;
        let d = ((cy + y) * out_w + cx) * 4;
        out.data[d..d + cw * 4].copy_from_slice(&scaled.data[s..s + cw * 4]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: usize, h: usize, v: f32) -> Image {
        Image::from_data(w, h, ChannelSet::Rgba, vec![v; w * h * 4]).unwrap()
    }

    #[test]
    fn flat_stays_exactly_flat_at_any_ratio() {
        let src = flat(97, 53, 0.4375);
        for (w, h) in [(32, 18), (200, 111), (97, 53), (1, 1), (13, 400)] {
            let out = resize(&src, w, h).unwrap();
            let worst = out
                .data
                .iter()
                .map(|v| (v - 0.4375).abs())
                .fold(0.0f32, f32::max);
            assert!(worst < 1e-6, "{w}x{h}: {worst}");
        }
    }

    #[test]
    fn shrinking_averages_detail_instead_of_aliasing() {
        // One-pixel stripes shrunk 4:1 must become the mean, not a sample.
        let (w, h) = (400, 40);
        let data: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                let v = if (i % w) % 2 == 0 { 1.0 } else { 0.0 };
                [v, v, v, 1.0]
            })
            .collect();
        let src = Image::from_data(w, h, ChannelSet::Rgba, data).unwrap();
        let out = resize(&src, 100, 10).unwrap();
        // Away from the edges — beyond the kernel's reach of three output
        // pixels at 4:1 — the stripes are exactly their mean. At the edge the
        // kernel is cut off and renormalised, which leaves a small bias there.
        let interior = (3..97)
            .map(|x| (out.get_rgba(x, 5)[0] - 0.5).abs())
            .fold(0.0f32, f32::max);
        assert!(
            interior < 1e-4,
            "stripes aliased: worst interior deviation {interior}"
        );
        let edge = (out.get_rgba(0, 5)[0] - 0.5).abs();
        assert!(edge < 0.05, "edge bias is bounded: {edge}");
    }

    #[test]
    fn shrinking_by_a_non_integer_ratio_does_not_beat() {
        // One-pixel stripes shrunk 3.7:1: any sampler that skips source pixels
        // shows a beat pattern here. The interior must be the flat mean.
        let (w, h) = (370, 8);
        let data: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                let v = if (i % w) % 2 == 0 { 1.0 } else { 0.0 };
                [v, v, v, 1.0]
            })
            .collect();
        let src = Image::from_data(w, h, ChannelSet::Rgba, data).unwrap();
        let out = resize(&src, 100, 2).unwrap();
        let interior = (4..96)
            .map(|x| (out.get_rgba(x, 1)[0] - 0.5).abs())
            .fold(0.0f32, f32::max);
        assert!(interior < 0.01, "beat pattern: {interior}");
    }

    #[test]
    fn same_shape_scales_and_different_shapes_need_a_decision() {
        let p = placement(1920, 1080, 1280, 720, Fit::Refuse).unwrap();
        assert_eq!(p.covers, (0, 0, 1280, 720));
        let err = placement(1920, 1080, 1080, 1080, Fit::Refuse).unwrap_err();
        assert!(err.contains("letterbox"), "{err}");

        let lb = placement(1920, 1080, 1080, 1080, Fit::Letterbox).unwrap();
        assert_eq!(lb.covers, (0, 236, 1080, 608));
        assert_eq!(lb.keeps, (0.0, 0.0, 1920.0, 1080.0));

        let crop = placement(1920, 1080, 1080, 1080, Fit::Crop).unwrap();
        assert_eq!(crop.covers, (0, 0, 1080, 1080));
        assert_eq!(crop.keeps, (420.0, 0.0, 1080.0, 1080.0));

        let st = placement(1920, 1080, 1080, 1080, Fit::Stretch).unwrap();
        assert_eq!(st.covers, (0, 0, 1080, 1080));
        assert!((st.scale.0 - 0.5625).abs() < 1e-12 && (st.scale.1 - 1.0).abs() < 1e-12);
    }

    #[test]
    fn letterbox_bars_are_the_background_and_the_picture_is_whole() {
        let src = flat(64, 36, 0.8);
        let p = placement(64, 36, 64, 64, Fit::Letterbox).unwrap();
        let out = fit(&src, 64, 64, &p, [0.0, 0.0, 0.0, 1.0]).unwrap();
        let (_, y0, _, h) = p.covers;
        assert_eq!((y0, h), (14, 36));
        assert_eq!(out.get_rgba(10, 5), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(out.get_rgba(10, 63), [0.0, 0.0, 0.0, 1.0]);
        assert!((out.get_rgba(10, 30)[0] - 0.8).abs() < 1e-6);
    }
}
