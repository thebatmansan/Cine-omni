//! The reference viewer: a decoded frame on the screen, honestly labelled.
//!
//! ## What "honestly" means here
//!
//! A decoded H.264 frame is planar YUV, and turning it into RGB requires
//! knowing which matrix encoded it. The reference asset — like a great deal of
//! real media — declares none. So the viewer resolves a matrix, records
//! **whether it was declared or assumed**, and exposes that as
//! [`FrameUploader::colour_note`] for the interface to display. A viewer that
//! silently picks a matrix teaches its user that the picture is authoritative
//! when it is a guess.
//!
//! The conversion itself is `cine-gpu`'s `yuv_to_rgb` kernel, which is checked
//! against a CPU reference on real hardware across BT.601/709/2020 and both
//! signal ranges.

use crate::{Result, ShellError};
use cine_gpu::device::{Arg, GpuContext};
use cine_gpu::kernels::KernelId;
use cine_gpu::yuv::{MatrixChoice, Range, YuvConversion};
use cine_media::VideoFrame;

/// Converts decoded frames into RGBA for display.
pub struct FrameUploader {
    conversion: Option<YuvConversion>,
    rgba: Vec<f32>,
    width: u32,
    height: u32,
}

impl Default for FrameUploader {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameUploader {
    pub fn new() -> FrameUploader {
        FrameUploader {
            conversion: None,
            rgba: Vec::new(),
            width: 0,
            height: 0,
        }
    }

    /// Convert one decoded frame to RGBA on the GPU.
    ///
    /// `ctx` is the shared compute context, so this runs on the same device the
    /// rest of the pipeline uses.
    pub fn upload(&mut self, ctx: &mut GpuContext, frame: &VideoFrame) -> Result<&[f32]> {
        let conversion = conversion_for(frame)?;
        let (y, u, v) = planes_as_f32(frame)?;

        let params = conversion.params();
        let bytes: Vec<u8> = params.iter().flat_map(|f| f.to_ne_bytes()).collect();
        let out_len = (frame.width * frame.height) as usize * 4;

        let mut results = ctx
            .dispatch(
                KernelId::YuvToRgb,
                &[
                    Arg::Uniform(&bytes),
                    Arg::Storage(&y),
                    Arg::Storage(&u),
                    Arg::Storage(&v),
                    Arg::Output(out_len),
                ],
                [frame.width.div_ceil(16), frame.height.div_ceil(16), 1],
            )
            .map_err(|e| ShellError::Gpu(e.to_string()))?;

        let raw = results
            .pop()
            .ok_or_else(|| ShellError::Gpu("no output buffer".into()))?;
        self.rgba = raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_ne_bytes(*b))
            .collect();
        self.width = frame.width;
        self.height = frame.height;
        self.conversion = Some(conversion);
        Ok(&self.rgba)
    }

    /// What the interface must show alongside the picture.
    ///
    /// `None` before the first frame. Otherwise a string that says whether the
    /// colour interpretation came from the file or from this build.
    pub fn colour_note(&self) -> Option<String> {
        let c = self.conversion?;
        Some(format!(
            "{} · {}",
            c.choice.describe(),
            match c.range {
                Range::Limited => "limited range",
                Range::Full => "full range",
            }
        ))
    }

    /// Whether the displayed picture rests on an assumption rather than on
    /// declared metadata. The interface is expected to make this visible.
    pub fn colour_is_assumed(&self) -> bool {
        self.conversion
            .map(|c| c.choice.is_assumed())
            .unwrap_or(false)
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn rgba(&self) -> &[f32] {
        &self.rgba
    }
}

/// Resolve how a frame must be interpreted, from what it actually declares.
pub fn conversion_for(frame: &VideoFrame) -> Result<YuvConversion> {
    if frame.format != cine_media::PixelFormat::Yuv420P8 {
        return Err(ShellError::Gpu(format!(
            "the viewer converts 4:2:0 8-bit only; this frame is {:?}",
            frame.format
        )));
    }
    let (cw, ch) = (frame.width.div_ceil(2), frame.height.div_ceil(2));
    Ok(YuvConversion {
        choice: MatrixChoice::resolve(frame.color.matrix, frame.height),
        // H.264 signals range in the VUI, which is not parsed. Limited is the
        // overwhelmingly common case for camera media and is the safer error:
        // reading full-range media as limited crushes it visibly, which is
        // noticed, whereas the reverse washes it out subtly.
        range: frame
            .color
            .full_range
            .map(|f| if f { Range::Full } else { Range::Limited })
            .unwrap_or(Range::Limited),
        width: frame.width,
        height: frame.height,
        chroma_width: cw,
        chroma_height: ch,
    })
}

/// Normalise the three 8-bit planes to 0..1, removing row padding.
fn planes_as_f32(frame: &VideoFrame) -> Result<(Vec<f32>, Vec<f32>, Vec<f32>)> {
    let plane = |i: usize, w: u32, h: u32| -> Result<Vec<f32>> {
        let (data, stride) = frame
            .plane(i)
            .ok_or_else(|| ShellError::Gpu(format!("frame has no plane {i}")))?;
        let (w, h) = (w as usize, h as usize);
        if data.len() < stride * (h - 1) + w {
            return Err(ShellError::Gpu(format!(
                "plane {i} is {} bytes, too small for {w}x{h} at stride {stride}",
                data.len()
            )));
        }
        Ok((0..h)
            .flat_map(|row| (0..w).map(move |col| data[row * stride + col] as f32 / 255.0))
            .collect())
    };
    let (cw, ch) = (frame.width.div_ceil(2), frame.height.div_ceil(2));
    Ok((
        plane(0, frame.width, frame.height)?,
        plane(1, cw, ch)?,
        plane(2, cw, ch)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cine_media::frame::{AlphaMode, ColorMetadata, PixelFormat};
    use cine_media::time::{TimeBase, Timestamp};
    use std::sync::Arc;

    fn frame(width: u32, height: u32, color: ColorMetadata) -> VideoFrame {
        let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
        VideoFrame {
            pts: Timestamp::new(0, TimeBase::new(1, 24).unwrap()),
            frame_index: None,
            timecode: None,
            width,
            height,
            format: PixelFormat::Yuv420P8,
            color,
            alpha: AlphaMode::None,
            planes: vec![
                (
                    Arc::new(vec![128u8; (width * height) as usize]),
                    width as usize,
                ),
                (Arc::new(vec![128u8; (cw * ch) as usize]), cw as usize),
                (Arc::new(vec![128u8; (cw * ch) as usize]), cw as usize),
            ],
        }
    }

    /// A frame with no colour metadata must still display — but the viewer must
    /// know, and be able to say, that it guessed.
    #[test]
    fn an_untagged_frame_is_displayed_but_marked_as_assumed() {
        let c = conversion_for(&frame(1920, 1080, ColorMetadata::default())).unwrap();
        assert!(c.choice.is_assumed());
        assert!(c.choice.describe().contains("ASSUMED"));
        assert_eq!(c.range, Range::Limited);
    }

    /// A frame that declares its matrix must be believed, not overridden.
    #[test]
    fn a_declared_matrix_is_used_and_reported_as_declared() {
        let color = ColorMetadata {
            matrix: Some(1),
            ..Default::default()
        };
        let c = conversion_for(&frame(720, 576, color)).unwrap();
        assert!(
            !c.choice.is_assumed(),
            "a declared matrix must not be second-guessed"
        );
        // 576 lines would otherwise fall to BT.601; the declaration wins.
        assert_eq!(c.choice.matrix(), cine_gpu::yuv::Matrix::BT709);
    }

    #[test]
    fn a_format_the_viewer_cannot_convert_is_refused() {
        let mut f = frame(64, 64, ColorMetadata::default());
        f.format = PixelFormat::Yuv422P8;
        assert!(
            conversion_for(&f).is_err(),
            "must refuse rather than misinterpret planes"
        );
    }

    /// Odd dimensions are legal in H.264 and the chroma planes round up.
    #[test]
    fn odd_dimensions_round_chroma_planes_up() {
        let c = conversion_for(&frame(1919, 1079, ColorMetadata::default())).unwrap();
        assert_eq!((c.chroma_width, c.chroma_height), (960, 540));
    }

    #[test]
    fn planes_are_normalised_and_padding_is_removed() {
        // A frame whose stride exceeds its width, as real decoders produce.
        let mut f = frame(4, 4, ColorMetadata::default());
        f.planes[0] = (Arc::new(vec![255u8; 8 * 4]), 8);
        let (y, _, _) = planes_as_f32(&f).unwrap();
        assert_eq!(
            y.len(),
            16,
            "padding must be stripped, not carried into the texture"
        );
        assert!(y.iter().all(|&v| (v - 1.0).abs() < 1e-6));
    }
}
