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

use cine_gpu::yuv::{yuv_to_rgb_cpu, YuvConversion};
use cine_media::{PixelFormat, VideoFrame, VideoReader};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};

/// One open decoder, reused between requests.
#[derive(Default)]
pub struct FrameService {
    open: Mutex<Option<(PathBuf, VideoReader)>>,
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
    pub fn picture(&self, path: &Path, index: usize) -> Result<Picture, String> {
        let began = std::time::Instant::now();
        let mut open = self.open.lock();
        let reusable = open
            .as_ref()
            .map(|(p, _)| p.as_path() == path)
            .unwrap_or(false);
        if !reusable {
            let reader = VideoReader::open_path(path).map_err(|e| e.to_string())?;
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

        let (png, width, height, colour, assumed) = encode(&frame)?;
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
    pub fn frame_count(&self, path: &Path) -> Result<usize, String> {
        let mut open = self.open.lock();
        let reusable = open
            .as_ref()
            .map(|(p, _)| p.as_path() == path)
            .unwrap_or(false);
        if !reusable {
            let reader = VideoReader::open_path(path).map_err(|e| e.to_string())?;
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
    if frame.format != PixelFormat::Yuv420P8 {
        return Err(format!(
            "this build converts 4:2:0 8-bit for preview; this picture is {:?}",
            frame.format
        ));
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

    let plane = |i: usize, pw: u32, ph: u32| -> Result<Vec<f32>, String> {
        let (data, stride) = frame
            .plane(i)
            .ok_or_else(|| format!("picture has no plane {i}"))?;
        let (pw, ph) = (pw as usize, ph as usize);
        if data.len() < stride * (ph - 1) + pw {
            return Err(format!("plane {i} is too small for {pw}x{ph}"));
        }
        Ok((0..ph)
            .flat_map(|row| (0..pw).map(move |col| f32::from(data[row * stride + col]) / 255.0))
            .collect())
    };
    let rgba = yuv_to_rgb_cpu(
        &conversion,
        &plane(0, w, h)?,
        &plane(1, cw, ch)?,
        &plane(2, cw, ch)?,
    );

    // 8-bit for transport: the browser has nowhere better to put it, and the
    // clamping is reported rather than silent.
    let bytes: Vec<u8> = rgba
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    Ok((
        bytes,
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
