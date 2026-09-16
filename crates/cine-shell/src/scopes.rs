//! Scopes of the picture on screen.
//!
//! A scope has to measure what the viewer is *showing*, not what was decoded:
//! a colourist reads it against the image in front of them. So the pixels here
//! go through the same two steps the GPU does — the YUV conversion
//! `cine-gpu` defines, then the view transform's lattice — and the scope is of
//! the result.
//!
//! Two honesties come with that:
//!
//! * The picture is **sampled**, not read whole: every `stride`-th pixel in
//!   each direction, so a 720×1280 frame costs a fraction of a frame's budget.
//!   [`Scopes::sampled`] says how many pixels were read, and the interface
//!   says so too. A scope of a sample is a scope of a sample.
//! * The lattice is the one the shader interpolates, so what the scope reads
//!   and what the eye sees differ by no more than the lattice's own error —
//!   which is measured (`cine-color`'s `view_lattice.rs`), not assumed.

use cine_color::lut::Lut3D;
use cine_color::scopes::{self as sc, ImageView};
use cine_gpu::yuv::{yuv_to_rgb_cpu, YuvConversion};
use cine_media::frame::VideoFrame;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How much of the picture a scope reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sampling {
    /// Every `stride`-th pixel in each direction.
    pub stride: usize,
}

impl Default for Sampling {
    fn default() -> Self {
        // Every fourth pixel each way: a sixteenth of the picture, which on a
        // 720×1280 frame is 57 600 samples — enough that a waveform column is
        // populated and small enough to compute between frames.
        Sampling { stride: 4 }
    }
}

/// The scopes of one picture.
#[derive(Clone, Debug)]
pub struct Scopes {
    pub waveform: sc::Waveform,
    pub parade: sc::Parade,
    pub vectorscope: sc::Vectorscope,
    pub histogram: sc::Histogram,
    /// Pixels actually read.
    pub sampled: usize,
    /// What computing them cost.
    pub took: Duration,
    /// Whether the view transform was applied before measuring.
    pub through_view: bool,
}

/// Compute the scopes of `frame` as the window is showing it.
///
/// `lut` is the view transform the GPU is interpolating; `None` means the
/// window is showing the decoded picture, and the scopes then measure that.
pub fn of_frame(
    frame: &VideoFrame,
    conversion: &YuvConversion,
    lut: Option<&Arc<Lut3D>>,
    sampling: Sampling,
    columns: usize,
    rows: usize,
) -> Option<Scopes> {
    let at = Instant::now();
    let (y, u, v) = (frame.plane(0)?, frame.plane(1)?, frame.plane(2)?);
    let stride = sampling.stride.max(1);
    let (w, h) = (conversion.width as usize, conversion.height as usize);
    let (cw, ch) = (
        conversion.chroma_width as usize,
        conversion.chroma_height as usize,
    );
    if w == 0 || h == 0 {
        return None;
    }

    // Sub-sample the planes first, then convert: converting the whole frame to
    // throw fifteen sixteenths of it away is the slow way round.
    let (sw, sh) = (w.div_ceil(stride), h.div_ceil(stride));
    let (scw, sch) = (cw.div_ceil(stride).max(1), ch.div_ceil(stride).max(1));
    let take = |plane: &[u8], pw: usize, ph: usize, ow: usize, oh: usize| -> Vec<f32> {
        let row_bytes = plane.len() / ph.max(1);
        let mut out = Vec::with_capacity(ow * oh);
        for oy in 0..oh {
            let sy = (oy * stride).min(ph.saturating_sub(1));
            for ox in 0..ow {
                let sx = (ox * stride).min(pw.saturating_sub(1));
                out.push(f32::from(plane[sy * row_bytes + sx]) / 255.0);
            }
        }
        out
    };
    let sampled_conversion = YuvConversion {
        width: sw as u32,
        height: sh as u32,
        chroma_width: scw as u32,
        chroma_height: sch as u32,
        ..*conversion
    };
    let mut rgba = yuv_to_rgb_cpu(
        &sampled_conversion,
        &take(y.0, w, h, sw, sh),
        &take(u.0, cw, ch, scw, sch),
        &take(v.0, cw, ch, scw, sch),
    );
    if let Some(lut) = lut {
        for px in rgba.as_chunks_mut::<4>().0 {
            let out = lut.apply([px[0], px[1], px[2]]);
            px[..3].copy_from_slice(&out);
        }
    }

    let view = ImageView::new(&rgba, sw, sh, 4);
    let scopes = Scopes {
        waveform: sc::waveform(view, columns, rows),
        parade: sc::rgb_parade(ImageView::new(&rgba, sw, sh, 4), columns / 3, rows),
        vectorscope: sc::vectorscope(ImageView::new(&rgba, sw, sh, 4), 128),
        histogram: sc::histogram(ImageView::new(&rgba, sw, sh, 4), 128),
        sampled: sw * sh,
        took: at.elapsed(),
        through_view: lut.is_some(),
    };
    Some(scopes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cine_gpu::yuv::{MatrixChoice, Range};
    use cine_media::frame::{AlphaMode, ColorMetadata, PixelFormat};
    use ucg_core::{Rate, RationalTime};

    /// A flat grey frame: limited-range Y' = 128, neutral chroma.
    fn grey_frame(w: usize, h: usize, luma: u8) -> VideoFrame {
        let y = vec![luma; w * h];
        let c = vec![128u8; (w / 2) * (h / 2)];
        VideoFrame {
            pts: RationalTime::zero(Rate::whole(24).expect("24 fps")),
            frame_index: Some(0),
            timecode: None,
            width: w as u32,
            height: h as u32,
            format: PixelFormat::Yuv420P8,
            color: ColorMetadata::default(),
            alpha: AlphaMode::None,
            planes: vec![
                (Arc::new(y), w),
                (Arc::new(c.clone()), w / 2),
                (Arc::new(c), w / 2),
            ],
        }
    }

    fn conversion(w: usize, h: usize) -> YuvConversion {
        YuvConversion {
            choice: MatrixChoice::resolve(Some(1), h as u32),
            range: Range::Limited,
            width: w as u32,
            height: h as u32,
            chroma_width: (w / 2) as u32,
            chroma_height: (h / 2) as u32,
        }
    }

    /// A known signal lands where it should: a flat grey reads as one level on
    /// the waveform, and the level follows the code value.
    #[test]
    fn a_flat_grey_reads_as_one_level_and_follows_the_signal() {
        let (w, h) = (64, 32);
        let mut levels = Vec::new();
        for luma in [64u8, 128, 200] {
            let frame = grey_frame(w, h, luma);
            let s = of_frame(
                &frame,
                &conversion(w, h),
                None,
                Sampling { stride: 2 },
                64,
                64,
            )
            .expect("scopes");
            assert_eq!(s.sampled, 32 * 16, "every second pixel each way");
            assert!(!s.through_view);
            // One populated row per column: the picture is flat.
            let column = 10;
            let hit: Vec<usize> = (0..s.waveform.rows)
                .filter(|r| sc::Waveform::at(&s.waveform.luma, s.waveform.rows, column, *r) > 0)
                .collect();
            assert_eq!(hit.len(), 1, "a flat picture is one line: {hit:?}");
            levels.push(hit[0]);
        }
        // Row 0 is the bottom of the scope, so a brighter signal reads higher.
        assert!(
            levels[0] < levels[1] && levels[1] < levels[2],
            "the waveform follows the signal: {levels:?}"
        );
    }

    /// The scope measures what is shown: with a view transform, it reads the
    /// transformed picture, not the decoded one.
    #[test]
    fn the_scope_reads_the_picture_through_the_view() {
        let (w, h) = (64, 32);
        let frame = grey_frame(w, h, 128);
        // 16 columns for the 16 sampled pixels across: a column per sample,
        // so every column carries a trace.
        let plain = of_frame(&frame, &conversion(w, h), None, Sampling::default(), 16, 64).unwrap();
        // A lattice that halves everything: the scope must move.
        let half = Lut3D::from_fn(2, |rgb| [rgb[0] * 0.5, rgb[1] * 0.5, rgb[2] * 0.5]);
        let half = Arc::new(half);
        let through = of_frame(
            &frame,
            &conversion(w, h),
            Some(&half),
            Sampling::default(),
            16,
            64,
        )
        .unwrap();
        assert!(through.through_view);
        let row_of = |s: &Scopes| {
            (0..s.waveform.rows)
                .find(|r| sc::Waveform::at(&s.waveform.luma, s.waveform.rows, 5, *r) > 0)
                .unwrap()
        };
        assert!(
            row_of(&through) < row_of(&plain),
            "halving the picture lowers the trace: {} vs {}",
            row_of(&through),
            row_of(&plain)
        );
    }
}
