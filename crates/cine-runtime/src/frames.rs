//! Decoded pictures, as images a browser can display.
//!
//! ## What this is, and what it is not
//!
//! This is **not** the playback path. The verified playback path decodes into
//! GPU textures and presents without the picture ever reaching the CPU —
//! `docs/blueprint/PLAYBACK.md` measures what happens when it does, and it is
//! 69.4 ms a picture. A browser cannot receive GPU textures, so a picture that
//! reaches one has, by construction, taken the slow road.
//!
//! What this *is*: real decoding of real media, one picture at a time, encoded
//! as PNG. Frame-accurate and honest. It is fast enough to look at a frame and
//! to step between frames; it is not real-time playback and the interface must
//! not present it as though it were.
//!
//! ## Why the reader is kept open
//!
//! Entering a compressed stream means finding the keyframe at or before the
//! wanted picture and decoding forward. On the reference asset, whose GOP is 189
//! pictures, a cold seek costs 4.8 seconds. Keeping the reader open lets the
//! next picture continue from where the last one stopped, which is what makes
//! stepping usable. One reader is cached: a second media file replaces it rather
//! than growing without bound.
//!
//! The conversion to RGB is `cine_gpu::yuv::yuv_to_rgb_cpu` — the same
//! definition the GPU path is verified against. A second conversion here would
//! be a second answer to "what colour is this picture".

use cine_gpu::yuv::{yuv8_to_rgb_cpu, YuvConversion};
use cine_media::{MediaFile, Pictures, PixelFormat, VideoFrame};
use parking_lot::Mutex;
use std::path::PathBuf;

/// One open decoder, reused between requests.
#[derive(Default)]
pub struct FrameService {
    open: Mutex<Option<(PathBuf, Pictures)>>,
}

/// What a picture request produced.
pub struct Picture {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Display index actually decoded.
    pub index: usize,
    /// Whether the colour interpretation had to be assumed.
    pub colour_assumed: bool,
    pub colour: String,
    pub decode_ms: f64,
}

impl FrameService {
    pub fn new() -> FrameService {
        FrameService::default()
    }

    /// Decode one picture and encode it as a PNG.
    ///
    /// `index` is a display index, clamped to the media. Errors are the
    /// decoder's own words: a codec this build cannot decode says so rather
    /// than returning a blank image.
    pub fn picture(&self, media: &MediaFile, index: usize) -> Result<Picture, String> {
        self.picture_through(media, index, None)
    }

    /// Decode one picture and encode it as a PNG, optionally through a view.
    ///
    /// With a view, the picture is transformed exactly as a render would
    /// transform it, and the colour description says which view it was. A
    /// scene-linear picture has no display values without one, so this is how
    /// an EXR is looked at honestly rather than clamped.
    pub fn picture_through(
        &self,
        media: &MediaFile,
        index: usize,
        view: Option<&cine_color::view::ViewTransform>,
    ) -> Result<Picture, String> {
        let began = std::time::Instant::now();
        let path = media.path.as_path();
        let mut open = self.open.lock();
        let reusable = open
            .as_ref()
            .map(|(p, _)| p.as_path() == path)
            .unwrap_or(false);
        if !reusable {
            let reader = Pictures::open(media.clone()).map_err(|e| e.to_string())?;
            *open = Some((path.to_path_buf(), reader));
        }
        let (_, reader) = open.as_mut().expect("just opened");

        let count = reader.frame_count();
        if count == 0 {
            return Err("the track has no pictures".into());
        }
        let index = index.min(count - 1);
        let frame = reader
            .frame_at_index(index)
            .map_err(|e| e.to_string())?
            .ok_or("no picture at that index")?;
        let decode_ms = began.elapsed().as_secs_f64() * 1e3;

        let (png, width, height, colour, assumed) = match view {
            None => encode(&frame)?,
            Some(view) => {
                let (mut px, w, h, decoded, assumed) = rgba_f32(&frame)?;
                view.apply_image(&mut px, 4);
                let bytes: Vec<u8> = px
                    .iter()
                    .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
                    .collect();
                (
                    png(&bytes, w, h)?,
                    w,
                    h,
                    format!("{decoded} · {}", view.describe()),
                    assumed,
                )
            }
        };
        Ok(Picture {
            png,
            width,
            height,
            index,
            colour_assumed: assumed,
            colour,
            decode_ms,
        })
    }

    /// How many pictures the media has, without decoding one.
    pub fn frame_count(&self, media: &MediaFile) -> Result<usize, String> {
        let path = media.path.as_path();
        let mut open = self.open.lock();
        let reusable = open
            .as_ref()
            .map(|(p, _)| p.as_path() == path)
            .unwrap_or(false);
        if !reusable {
            let reader = Pictures::open(media.clone()).map_err(|e| e.to_string())?;
            *open = Some((path.to_path_buf(), reader));
        }
        Ok(open.as_ref().map(|(_, r)| r.frame_count()).unwrap_or(0))
    }
}

/// YUV planes to PNG, through the verified reference conversion.
fn encode(frame: &VideoFrame) -> Result<(Vec<u8>, u32, u32, String, bool), String> {
    let (bytes, w, h, colour, assumed) = rgba8(frame)?;
    Ok((png(&bytes, w, h)?, w, h, colour, assumed))
}

/// PNG bytes for 8-bit RGBA.
pub fn png(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    {
        let encoder = image::codecs::png::PngEncoder::new(&mut png);
        image::ImageEncoder::write_image(encoder, rgba, w, h, image::ExtendedColorType::Rgba8)
            .map_err(|e| e.to_string())?;
    }
    Ok(png)
}

/// 8-bit RGBA through the verified reference conversion, with a description of
/// the colour interpretation and whether it was assumed.
pub fn rgba8(frame: &VideoFrame) -> Result<(Vec<u8>, u32, u32, String, bool), String> {
    let (rgba, w, h, describe, assumed) = rgba_f32(frame)?;
    // A float picture holds scene values outside [0, 1]; showing it as 8-bit
    // clamps them, and the viewer is told so rather than shown a clipped
    // picture described as the file's.
    let describe = match frame.format {
        PixelFormat::RgbaF32 => format!("{describe} · shown clamped to [0, 1], no view applied"),
        _ => describe,
    };
    // 8-bit for transport: the browser has nowhere better to put it, and the
    // clamping is reported rather than silent.
    let bytes: Vec<u8> = rgba
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    Ok((bytes, w, h, describe, assumed))
}

/// RGBA code values through the verified reference conversion, unclamped:
/// limited-range video reaches below 0 and above 1, and a view transform is
/// entitled to see those excursions before anything clips them.
pub fn rgba_f32(frame: &VideoFrame) -> Result<(Vec<f32>, u32, u32, String, bool), String> {
    // Pictures that are already RGB arrive from image sequences at the
    // precision they were stored: 8-bit, 16-bit, or 32-bit float, the last
    // unclamped because scene values live outside [0, 1]. No matrix and no
    // range conversion apply to them, and none is invented.
    match frame.format {
        PixelFormat::Rgba8 | PixelFormat::Rgba16 | PixelFormat::RgbaF32 => {
            return rgba_f32_direct(frame);
        }
        PixelFormat::Yuv420P8 => {}
        other => {
            return Err(format!(
                "this build converts 4:2:0 8-bit and RGBA pictures; this picture is {other:?}"
            ))
        }
    }
    let (w, h) = (frame.width, frame.height);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let conversion = YuvConversion {
        choice: cine_gpu::yuv::MatrixChoice::resolve(frame.color.matrix, h),
        range: frame
            .color
            .full_range
            .map(|f| {
                if f {
                    cine_gpu::yuv::Range::Full
                } else {
                    cine_gpu::yuv::Range::Limited
                }
            })
            .unwrap_or(cine_gpu::yuv::Range::Limited),
        width: w,
        height: h,
        chroma_width: cw,
        chroma_height: ch,
    };

    // Straight from the decoder's planes: same arithmetic as the reference
    // conversion, checked against it in cine-gpu.
    let mut planes: [&[u8]; 3] = [&[], &[], &[]];
    let mut strides = [0usize; 3];
    for i in 0..3 {
        let (pw, ph) = if i == 0 {
            (w as usize, h as usize)
        } else {
            (cw as usize, ch as usize)
        };
        let (data, stride) = frame
            .plane(i)
            .ok_or_else(|| format!("picture has no plane {i}"))?;
        if data.len() < stride * (ph - 1) + pw {
            return Err(format!("plane {i} is too small for {pw}x{ph}"));
        }
        planes[i] = data;
        strides[i] = stride;
    }
    let rgba = yuv8_to_rgb_cpu(&conversion, planes, strides);

    Ok((
        rgba,
        w,
        h,
        format!(
            "{} · {}",
            conversion.choice.describe(),
            match conversion.range {
                cine_gpu::yuv::Range::Limited => "limited range",
                cine_gpu::yuv::Range::Full => "full range",
            }
        ),
        conversion.choice.is_assumed(),
    ))
}

/// An RGBA picture as float samples, at the precision it holds.
fn rgba_f32_direct(frame: &VideoFrame) -> Result<(Vec<f32>, u32, u32, String, bool), String> {
    let (w, h) = (frame.width, frame.height);
    let (bytes, stride) = frame.plane(0).ok_or("the picture has no plane")?;
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    let describe = match frame.format {
        PixelFormat::Rgba8 => {
            for row in 0..h as usize {
                let line = &bytes[row * stride..row * stride + w as usize * 4];
                out.extend(line.iter().map(|v| f32::from(*v) / 255.0));
            }
            "8-bit RGBA"
        }
        PixelFormat::Rgba16 => {
            for row in 0..h as usize {
                let line = &bytes[row * stride..row * stride + w as usize * 8];
                out.extend(
                    line.as_chunks::<2>()
                        .0
                        .iter()
                        .map(|b| f32::from(u16::from_ne_bytes(*b)) / 65535.0),
                );
            }
            "16-bit RGBA"
        }
        PixelFormat::RgbaF32 => {
            for row in 0..h as usize {
                let line = &bytes[row * stride..row * stride + w as usize * 16];
                out.extend(
                    line.as_chunks::<4>()
                        .0
                        .iter()
                        .map(|b| f32::from_ne_bytes(*b)),
                );
            }
            "32-bit float RGBA, unclamped"
        }
        other => return Err(format!("{other:?} is not an RGBA picture")),
    };
    Ok((out, w, h, describe.to_string(), false))
}
