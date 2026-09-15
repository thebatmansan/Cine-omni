//! Scene-linear float image buffers.
//!
//! * [`Image`] is a planar-interleaved `f32` buffer with a [`ChannelSet`].
//!   RGBA images use the **premultiplied alpha** convention throughout the
//!   compositor.
//! * [`MultiLayerImage`] groups named AOV layers from a render (beauty,
//!   depth, normal, id, motion, …) the way a multi-part EXR does.
//! * [`DeepImage`] stores a list of depth-sorted samples per pixel for deep
//!   compositing.

use indexmap::IndexMap;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// Which channels an [`Image`] carries, in memory order.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChannelSet {
    /// Premultiplied red, green, blue, alpha.
    Rgba,
    /// Red, green, blue (alpha implicitly one).
    Rgb,
    /// Single alpha / matte channel.
    Alpha,
    /// Single camera-space depth (Z) channel.
    Depth,
    /// Surface normal XYZ.
    Normal,
    /// Screen-space motion vector XY.
    Motion,
    /// Texture coordinates UV.
    Uv,
    /// Arbitrary named channels (AOVs, cryptomatte ids, …).
    Named(Vec<String>),
}

impl ChannelSet {
    /// Number of floats per pixel.
    pub fn count(&self) -> usize {
        match self {
            ChannelSet::Rgba => 4,
            ChannelSet::Rgb => 3,
            ChannelSet::Alpha | ChannelSet::Depth => 1,
            ChannelSet::Normal => 3,
            ChannelSet::Motion | ChannelSet::Uv => 2,
            ChannelSet::Named(n) => n.len(),
        }
    }

    /// Channel names in memory order.
    pub fn names(&self) -> Vec<String> {
        let fixed: &[&str] = match self {
            ChannelSet::Rgba => &["r", "g", "b", "a"],
            ChannelSet::Rgb => &["r", "g", "b"],
            ChannelSet::Alpha => &["a"],
            ChannelSet::Depth => &["z"],
            ChannelSet::Normal => &["nx", "ny", "nz"],
            ChannelSet::Motion => &["mx", "my"],
            ChannelSet::Uv => &["u", "v"],
            ChannelSet::Named(n) => return n.clone(),
        };
        fixed.iter().map(|s| s.to_string()).collect()
    }

    /// Index of a channel by name.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.names().iter().position(|n| n == name)
    }

    /// True when this set has a dedicated alpha channel.
    pub fn has_alpha(&self) -> bool {
        matches!(self, ChannelSet::Rgba | ChannelSet::Alpha) || self.index_of("a").is_some()
    }
}

/// Encode a scene-linear value to sRGB display (no tone mapping).
pub fn srgb_encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Decode an sRGB display value to scene-linear.
pub fn srgb_decode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// A scene-linear float image. Row-major, interleaved channels.
///
/// ## The data window
///
/// `width × height` is the size of the **buffer**, and `(x, y)` is where that
/// buffer sits on the canvas. Everything outside it is transparent black. A
/// full frame has its data window at the origin, which is what every image
/// had before the window existed, so full-frame work is unchanged.
///
/// The window is what lets a region be computed without allocating the frame:
/// asked for 64×64 of a 2048×2048 comp, a node produces a 64×64 buffer placed at
/// the region, not a 2048×2048 buffer that is zero almost everywhere.
///
/// Pixel accessors (`get`, `pixel`, `get_rgba` …) take **buffer** coordinates.
/// [`Image::canvas_rgba`] and [`Image::crop_canvas`] take canvas coordinates,
/// and anything that combines two images must go through those — or through
/// [`Image::aligned_to`], which puts one image on another's window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub channels: ChannelSet,
    pub data: Vec<f32>,
    /// Canvas position of the buffer's first pixel.
    #[serde(default)]
    pub x: i64,
    #[serde(default)]
    pub y: i64,
}

impl Default for Image {
    fn default() -> Self {
        Image::new(0, 0, ChannelSet::Rgba)
    }
}

impl Image {
    /// Zero-filled image.
    pub fn new(width: usize, height: usize, channels: ChannelSet) -> Self {
        let n = width * height * channels.count();
        Image {
            width,
            height,
            channels,
            data: vec![0.0; n],
            x: 0,
            y: 0,
        }
    }

    /// Zero-filled RGBA image.
    pub fn rgba(width: usize, height: usize) -> Self {
        Image::new(width, height, ChannelSet::Rgba)
    }

    /// Wrap an existing buffer; the length must match.
    pub fn from_data(
        width: usize,
        height: usize,
        channels: ChannelSet,
        data: Vec<f32>,
    ) -> Option<Self> {
        if data.len() != width * height * channels.count() {
            return None;
        }
        Some(Image {
            width,
            height,
            channels,
            data,
            x: 0,
            y: 0,
        })
    }

    /// Constant-color RGBA image (`color` is premultiplied as given).
    pub fn solid(width: usize, height: usize, color: [f32; 4]) -> Self {
        let mut img = Image::rgba(width, height);
        for px in img.data.as_chunks_mut::<4>().0 {
            px.copy_from_slice(&color);
        }
        img
    }

    /// A generator's pixels over only `window` of a `width × height` frame,
    /// placed there. `pixel(x, y)` receives canvas coordinates. Without a
    /// window, the whole frame.
    pub fn generate_window(
        width: usize,
        height: usize,
        window: Option<RegionOfInterest>,
        pixel: impl Fn(usize, usize) -> [f32; 4],
    ) -> Image {
        let w = window
            .map(|r| r.clipped(width, height))
            .unwrap_or(RegionOfInterest::new(0, 0, width, height));
        let mut img = Image::rgba(w.width, w.height).placed(w.x as i64, w.y as i64);
        for y in 0..w.height {
            for x in 0..w.width {
                img.set_rgba(x, y, pixel(w.x + x, w.y + y));
            }
        }
        img
    }

    /// Checkerboard of `size`-pixel squares alternating `c1` / `c2`.
    pub fn checkerboard(
        width: usize,
        height: usize,
        size: usize,
        c1: [f32; 4],
        c2: [f32; 4],
    ) -> Self {
        let size = size.max(1);
        let mut img = Image::rgba(width, height);
        for y in 0..height {
            for x in 0..width {
                let c = if ((x / size) + (y / size)).is_multiple_of(2) {
                    c1
                } else {
                    c2
                };
                img.set_rgba(x, y, c);
            }
        }
        img
    }

    /// Linear ramp from `from` (left/top) to `to` (right/bottom).
    pub fn ramp(
        width: usize,
        height: usize,
        horizontal: bool,
        from: [f32; 4],
        to: [f32; 4],
    ) -> Self {
        let mut img = Image::rgba(width, height);
        for y in 0..height {
            for x in 0..width {
                let t = if horizontal {
                    if width > 1 {
                        x as f32 / (width - 1) as f32
                    } else {
                        0.0
                    }
                } else if height > 1 {
                    y as f32 / (height - 1) as f32
                } else {
                    0.0
                };
                let mut c = [0.0; 4];
                for (i, v) in c.iter_mut().enumerate() {
                    *v = from[i] + (to[i] - from[i]) * t;
                }
                img.set_rgba(x, y, c);
            }
        }
        img
    }

    /// Floats per pixel.
    pub fn nc(&self) -> usize {
        self.channels.count()
    }

    /// Number of pixels.
    pub fn pixel_count(&self) -> usize {
        self.width * self.height
    }

    /// True when the image has no pixels.
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// True if `x, y` is inside the image.
    pub fn contains(&self, x: i64, y: i64) -> bool {
        x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height
    }

    /// Whether `other` has the same dimensions.
    pub fn same_size(&self, other: &Image) -> bool {
        self.width == other.width && self.height == other.height
    }

    fn idx(&self, x: usize, y: usize) -> usize {
        (y * self.width + x) * self.nc()
    }

    /// All channels of one pixel.
    pub fn pixel(&self, x: usize, y: usize) -> &[f32] {
        let i = self.idx(x, y);
        &self.data[i..i + self.nc()]
    }

    /// Mutable channels of one pixel.
    pub fn pixel_mut(&mut self, x: usize, y: usize) -> &mut [f32] {
        let nc = self.nc();
        let i = self.idx(x, y);
        &mut self.data[i..i + nc]
    }

    /// Set all channels of one pixel (extra values ignored, missing left unchanged).
    pub fn set_pixel(&mut self, x: usize, y: usize, values: &[f32]) {
        let px = self.pixel_mut(x, y);
        for (d, s) in px.iter_mut().zip(values) {
            *d = *s;
        }
    }

    /// One channel value of one pixel.
    pub fn get(&self, x: usize, y: usize, c: usize) -> f32 {
        self.data[self.idx(x, y) + c]
    }

    /// Set one channel value of one pixel.
    pub fn set(&mut self, x: usize, y: usize, c: usize, v: f32) {
        let i = self.idx(x, y) + c;
        self.data[i] = v;
    }

    /// Promote a raw pixel of this channel set to RGBA.
    pub fn promote_rgba(&self, px: &[f32]) -> [f32; 4] {
        match &self.channels {
            ChannelSet::Rgba => [px[0], px[1], px[2], px[3]],
            ChannelSet::Rgb | ChannelSet::Normal => [px[0], px[1], px[2], 1.0],
            ChannelSet::Alpha => [px[0], px[0], px[0], px[0]],
            ChannelSet::Depth => [px[0], px[0], px[0], 1.0],
            ChannelSet::Motion | ChannelSet::Uv => [px[0], px[1], 0.0, 1.0],
            ChannelSet::Named(_) => {
                let mut out = [0.0, 0.0, 0.0, 1.0];
                for (i, v) in px.iter().take(4).enumerate() {
                    out[i] = *v;
                }
                out
            }
        }
    }

    /// RGBA value of a pixel, promoting other channel sets (RGB gets alpha 1,
    /// single channels are replicated, depth/normal/motion map to RGB with alpha 1).
    pub fn get_rgba(&self, x: usize, y: usize) -> [f32; 4] {
        self.promote_rgba(self.pixel(x, y))
    }

    /// Store an RGBA value into a pixel (demoting to the channel set).
    pub fn set_rgba(&mut self, x: usize, y: usize, c: [f32; 4]) {
        match &self.channels {
            ChannelSet::Rgba => self.set_pixel(x, y, &c),
            ChannelSet::Rgb | ChannelSet::Normal => self.set_pixel(x, y, &c[..3]),
            ChannelSet::Alpha => self.set_pixel(x, y, &[c[3]]),
            ChannelSet::Depth => self.set_pixel(x, y, &[c[0]]),
            ChannelSet::Motion | ChannelSet::Uv => self.set_pixel(x, y, &c[..2]),
            ChannelSet::Named(_) => self.set_pixel(x, y, &c),
        }
    }

    /// Alpha of a pixel (1 for sets without alpha, the value for single channels).
    pub fn get_alpha(&self, x: usize, y: usize) -> f32 {
        self.get_rgba(x, y)[3]
    }

    /// Convert to an RGBA image (clone if already RGBA).
    pub fn to_rgba(&self) -> Image {
        if self.channels == ChannelSet::Rgba {
            return self.clone();
        }
        let mut out = Image::rgba(self.width, self.height).placed(self.x, self.y);
        let nc = self.nc();
        out.data
            .par_chunks_exact_mut(4)
            .zip(self.data.par_chunks_exact(nc))
            .for_each(|(d, s)| d.copy_from_slice(&self.promote_rgba(s)));
        out
    }

    /// Extract the alpha as a single-channel image.
    pub fn alpha_image(&self) -> Image {
        let mut out = Image::new(self.width, self.height, ChannelSet::Alpha).placed(self.x, self.y);
        for y in 0..self.height {
            for x in 0..self.width {
                out.set(x, y, 0, self.get_alpha(x, y));
            }
        }
        out
    }

    /// Row-major iterator over rows.
    pub fn rows(&self) -> std::slice::ChunksExact<'_, f32> {
        self.data.chunks_exact((self.width * self.nc()).max(1))
    }

    /// Mutable rows.
    pub fn rows_mut(&mut self) -> std::slice::ChunksExactMut<'_, f32> {
        let stride = (self.width * self.nc()).max(1);
        self.data.chunks_exact_mut(stride)
    }

    /// Apply a per-pixel RGBA function in parallel, producing an RGBA image.
    pub fn map_rgba<F>(&self, f: F) -> Image
    where
        F: Fn([f32; 4]) -> [f32; 4] + Sync,
    {
        let src = self.to_rgba();
        let mut out = Image::rgba(self.width, self.height).placed(self.x, self.y);
        out.data
            .par_chunks_exact_mut(4)
            .zip(src.data.par_chunks_exact(4))
            .for_each(|(d, s)| {
                let r = f([s[0], s[1], s[2], s[3]]);
                d.copy_from_slice(&r);
            });
        out
    }

    /// Apply a per-pixel function that also receives the pixel coordinates.
    pub fn map_rgba_xy<F>(&self, f: F) -> Image
    where
        F: Fn(usize, usize, [f32; 4]) -> [f32; 4] + Sync,
    {
        let src = self.to_rgba();
        let w = self.width;
        let mut out = Image::rgba(self.width, self.height).placed(self.x, self.y);
        out.data
            .par_chunks_exact_mut(4 * w.max(1))
            .zip(src.data.par_chunks_exact(4 * w.max(1)))
            .enumerate()
            .for_each(|(y, (drow, srow))| {
                for (x, (d, s)) in drow
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(srow.as_chunks::<4>().0)
                    .enumerate()
                {
                    d.copy_from_slice(&f(x, y, [s[0], s[1], s[2], s[3]]));
                }
            });
        out
    }

    /// Crop to the rectangle `[x, x+w) × [y, y+h)`; pixels outside the source are zero.
    pub fn crop(&self, x: i64, y: i64, w: usize, h: usize) -> Image {
        let nc = self.nc();
        let mut out = Image::new(w, h, self.channels.clone());
        for oy in 0..h {
            let sy = y + oy as i64;
            if sy < 0 || sy as usize >= self.height {
                continue;
            }
            for ox in 0..w {
                let sx = x + ox as i64;
                if sx < 0 || sx as usize >= self.width {
                    continue;
                }
                let si = self.idx(sx as usize, sy as usize);
                let oi = (oy * w + ox) * nc;
                out.data[oi..oi + nc].copy_from_slice(&self.data[si..si + nc]);
            }
        }
        out
    }

    /// The same image, placed with its first pixel at canvas `(x, y)`.
    pub fn placed(mut self, x: i64, y: i64) -> Image {
        self.x = x;
        self.y = y;
        self
    }

    /// An empty buffer of `channels` on the same data window as `self`.
    pub fn blank_like(&self, channels: ChannelSet) -> Image {
        Image::new(self.width, self.height, channels).placed(self.x, self.y)
    }

    /// The data window as `(x, y, width, height)` on the canvas.
    pub fn window(&self) -> (i64, i64, usize, usize) {
        (self.x, self.y, self.width, self.height)
    }

    /// RGBA at canvas coordinates; transparent black outside the data window.
    pub fn canvas_rgba(&self, cx: i64, cy: i64) -> [f32; 4] {
        let (bx, by) = (cx - self.x, cy - self.y);
        if self.contains(bx, by) {
            self.get_rgba(bx as usize, by as usize)
        } else {
            [0.0; 4]
        }
    }

    /// The canvas rectangle `[x, x+w) × [y, y+h)` as a new image placed there;
    /// pixels outside this image's data window are zero.
    pub fn crop_canvas(&self, x: i64, y: i64, w: usize, h: usize) -> Image {
        self.crop(x - self.x, y - self.y, w, h).placed(x, y)
    }

    /// This image on `other`'s data window: the same canvas pixels, cropped and
    /// padded with transparent black to match. A clone when already aligned.
    pub fn aligned_to(&self, other: &Image) -> Image {
        if self.window() == other.window() {
            self.clone()
        } else {
            self.crop_canvas(other.x, other.y, other.width, other.height)
        }
    }

    /// Bilinear sample of all channels at continuous pixel coordinates
    /// (integer coordinates hit pixel centers exactly). Clamps to the edge.
    pub fn sample_bilinear_into(&self, x: f32, y: f32, out: &mut [f32]) {
        if self.is_empty() {
            out.iter_mut().for_each(|v| *v = 0.0);
            return;
        }
        let maxx = (self.width - 1) as f32;
        let maxy = (self.height - 1) as f32;
        let x = x.clamp(0.0, maxx);
        let y = y.clamp(0.0, maxy);
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let fx = x - x0 as f32;
        let fy = y - y0 as f32;
        let p00 = self.pixel(x0, y0);
        let p10 = self.pixel(x1, y0);
        let p01 = self.pixel(x0, y1);
        let p11 = self.pixel(x1, y1);
        for (c, o) in out.iter_mut().enumerate().take(self.nc()) {
            let top = p00[c] + (p10[c] - p00[c]) * fx;
            let bot = p01[c] + (p11[c] - p01[c]) * fx;
            *o = top + (bot - top) * fy;
        }
    }

    /// Bilinear sample promoted to RGBA.
    pub fn sample_bilinear(&self, x: f32, y: f32) -> [f32; 4] {
        let mut buf = [0.0f32; 16];
        let nc = self.nc().min(16);
        self.sample_bilinear_into(x, y, &mut buf[..nc]);
        self.promote_rgba(&buf[..nc])
    }

    /// Catmull-Rom bicubic sample promoted to RGBA (edge-clamped).
    pub fn sample_bicubic(&self, x: f32, y: f32) -> [f32; 4] {
        if self.is_empty() {
            return [0.0; 4];
        }
        fn w(t: f32) -> [f32; 4] {
            let t2 = t * t;
            let t3 = t2 * t;
            [
                -0.5 * t3 + t2 - 0.5 * t,
                1.5 * t3 - 2.5 * t2 + 1.0,
                -1.5 * t3 + 2.0 * t2 + 0.5 * t,
                0.5 * t3 - 0.5 * t2,
            ]
        }
        let maxx = (self.width - 1) as f32;
        let maxy = (self.height - 1) as f32;
        let x = x.clamp(0.0, maxx);
        let y = y.clamp(0.0, maxy);
        let x0 = x.floor();
        let y0 = y.floor();
        let wx = w(x - x0);
        let wy = w(y - y0);
        let mut acc = [0.0f32; 4];
        for (j, wyj) in wy.iter().enumerate() {
            let sy = ((y0 as i64) + j as i64 - 1).clamp(0, self.height as i64 - 1) as usize;
            for (i, wxi) in wx.iter().enumerate() {
                let sx = ((x0 as i64) + i as i64 - 1).clamp(0, self.width as i64 - 1) as usize;
                let p = self.get_rgba(sx, sy);
                let wt = wxi * wyj;
                for (a, v) in acc.iter_mut().zip(p) {
                    *a += v * wt;
                }
            }
        }
        acc
    }

    /// Bilinear resize to `w × h`.
    pub fn resize(&self, w: usize, h: usize) -> Image {
        let nc = self.nc();
        let mut out = Image::new(w, h, self.channels.clone());
        if self.is_empty() || w == 0 || h == 0 {
            return out;
        }
        let sx = self.width as f32 / w as f32;
        let sy = self.height as f32 / h as f32;
        out.data
            .par_chunks_exact_mut(w * nc)
            .enumerate()
            .for_each(|(y, row)| {
                let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
                for (x, px) in row.chunks_exact_mut(nc).enumerate() {
                    let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
                    self.sample_bilinear_into(fx, fy, px);
                }
            });
        out
    }

    /// 8-bit RGBA display buffer: RGB sRGB-encoded, alpha linear.
    pub fn to_display_u8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixel_count() * 4);
        for y in 0..self.height {
            for x in 0..self.width {
                let c = self.get_rgba(x, y);
                for v in &c[..3] {
                    out.push((srgb_encode(*v) * 255.0 + 0.5) as u8);
                }
                out.push((c[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
        }
        out
    }

    /// Mean of one channel.
    pub fn channel_mean(&self, c: usize) -> f64 {
        if self.is_empty() {
            return 0.0;
        }
        let nc = self.nc();
        let sum: f64 = self.data.chunks_exact(nc).map(|p| p[c] as f64).sum();
        sum / self.pixel_count() as f64
    }

    /// Sum of one channel.
    pub fn channel_sum(&self, c: usize) -> f64 {
        let nc = self.nc();
        self.data.chunks_exact(nc).map(|p| p[c] as f64).sum()
    }

    /// Largest absolute per-value difference between two same-shaped images.
    pub fn max_abs_diff(&self, other: &Image) -> f32 {
        if self.data.len() != other.data.len() {
            return f32::INFINITY;
        }
        self.data
            .iter()
            .zip(&other.data)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max)
    }

    /// Element-wise closeness test.
    pub fn approx_eq(&self, other: &Image, eps: f32) -> bool {
        self.same_size(other) && self.channels == other.channels && self.max_abs_diff(other) <= eps
    }
}

/// One named AOV layer of a render.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    pub image: Image,
}

/// A multi-layer render output (beauty, depth, normal, id, motion, …),
/// modelled on multi-part EXR.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MultiLayerImage {
    pub width: usize,
    pub height: usize,
    pub layers: IndexMap<String, Image>,
}

impl MultiLayerImage {
    /// Empty container for `width × height` layers.
    pub fn new(width: usize, height: usize) -> Self {
        MultiLayerImage {
            width,
            height,
            layers: IndexMap::new(),
        }
    }

    /// Insert or replace a layer (size must match; returns false otherwise).
    pub fn insert(&mut self, name: impl Into<String>, image: Image) -> bool {
        if image.width != self.width || image.height != self.height {
            return false;
        }
        self.layers.insert(name.into(), image);
        true
    }

    /// Look up a layer by name.
    pub fn layer(&self, name: &str) -> Option<&Image> {
        self.layers.get(name)
    }

    /// Conventional layer accessors.
    pub fn beauty(&self) -> Option<&Image> {
        self.layer("beauty").or_else(|| self.layer("rgba"))
    }
    /// The depth (Z) layer.
    pub fn depth(&self) -> Option<&Image> {
        self.layer("depth").or_else(|| self.layer("Z"))
    }
    /// The normal layer.
    pub fn normal(&self) -> Option<&Image> {
        self.layer("normal")
    }
    /// The object/cryptomatte id layer.
    pub fn id(&self) -> Option<&Image> {
        self.layer("id").or_else(|| self.layer("crypto"))
    }
    /// The motion vector layer.
    pub fn motion(&self) -> Option<&Image> {
        self.layer("motion")
    }

    /// Layer names in insertion order.
    pub fn layer_names(&self) -> Vec<&str> {
        self.layers.keys().map(String::as_str).collect()
    }

    /// Owned layers as a list.
    pub fn into_layers(self) -> Vec<Layer> {
        self.layers
            .into_iter()
            .map(|(name, image)| Layer { name, image })
            .collect()
    }
}

/// One deep sample: a premultiplied RGBA fragment with a depth interval.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct DeepSample {
    pub z: f32,
    pub z_back: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl DeepSample {
    /// A point sample (`z_back == z`).
    pub fn point(z: f32, rgba: [f32; 4]) -> Self {
        DeepSample {
            z,
            z_back: z,
            r: rgba[0],
            g: rgba[1],
            b: rgba[2],
            a: rgba[3],
        }
    }
    /// The color as an RGBA array.
    pub fn rgba(&self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
    /// Scale the fragment (all four channels) by `t`.
    pub fn scaled(mut self, t: f32) -> Self {
        self.r *= t;
        self.g *= t;
        self.b *= t;
        self.a *= t;
        self
    }
}

/// A deep image: a list of samples per pixel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeepImage {
    pub width: usize,
    pub height: usize,
    /// `samples[y * width + x]` is the sample list of pixel `(x, y)`.
    pub samples: Vec<Vec<DeepSample>>,
}

impl DeepImage {
    /// Empty deep image (no samples anywhere).
    pub fn new(width: usize, height: usize) -> Self {
        DeepImage {
            width,
            height,
            samples: vec![Vec::new(); width * height],
        }
    }

    /// Samples of one pixel.
    pub fn pixel(&self, x: usize, y: usize) -> &[DeepSample] {
        &self.samples[y * self.width + x]
    }

    /// Mutable samples of one pixel.
    pub fn pixel_mut(&mut self, x: usize, y: usize) -> &mut Vec<DeepSample> {
        &mut self.samples[y * self.width + x]
    }

    /// Append a sample to a pixel.
    pub fn push(&mut self, x: usize, y: usize, s: DeepSample) {
        self.pixel_mut(x, y).push(s);
    }

    /// Total number of samples.
    pub fn sample_count(&self) -> usize {
        self.samples.iter().map(Vec::len).sum()
    }

    /// Sort every pixel's samples front to back.
    pub fn sort(&mut self) {
        self.samples.par_iter_mut().for_each(|s| {
            s.sort_by(|a, b| a.z.partial_cmp(&b.z).unwrap_or(std::cmp::Ordering::Equal));
        });
    }

    /// Flatten with front-to-back premultiplied over compositing.
    pub fn flatten(&self) -> Image {
        let mut out = Image::rgba(self.width, self.height);
        out.data
            .par_chunks_exact_mut(4)
            .zip(self.samples.par_iter())
            .for_each(|(px, list)| {
                let mut sorted: Vec<&DeepSample> = list.iter().collect();
                sorted.sort_by(|a, b| a.z.partial_cmp(&b.z).unwrap_or(std::cmp::Ordering::Equal));
                let mut acc = [0.0f32; 4];
                for s in sorted {
                    let t = 1.0 - acc[3];
                    acc[0] += s.r * t;
                    acc[1] += s.g * t;
                    acc[2] += s.b * t;
                    acc[3] += s.a * t;
                }
                px.copy_from_slice(&acc);
            });
        out
    }

    /// Merge the samples of two same-sized deep images (sorted result).
    pub fn merged(&self, other: &DeepImage) -> Option<DeepImage> {
        if self.width != other.width || self.height != other.height {
            return None;
        }
        let mut out = self.clone();
        for (dst, src) in out.samples.iter_mut().zip(&other.samples) {
            dst.extend_from_slice(src);
        }
        out.sort();
        Some(out)
    }

    /// Build a one-sample-per-pixel deep image from a flat RGBA image and a depth image.
    pub fn from_image(image: &Image, depth: &Image) -> Option<DeepImage> {
        if !image.same_size(depth) {
            return None;
        }
        let mut out = DeepImage::new(image.width, image.height);
        for y in 0..image.height {
            for x in 0..image.width {
                let c = image.get_rgba(x, y);
                let z = depth.pixel(x, y)[0];
                if c[3] > 0.0 || c[0] != 0.0 || c[1] != 0.0 || c[2] != 0.0 {
                    out.push(x, y, DeepSample::point(z, c));
                }
            }
        }
        Some(out)
    }

    /// Hold out this image by `holdout`: every sample is attenuated by the
    /// transmittance of the holdout samples in front of it, and samples that
    /// end up fully covered (transmittance `<= eps`) are removed.
    pub fn holdout(&self, holdout: &DeepImage, eps: f32) -> Option<DeepImage> {
        if self.width != holdout.width || self.height != holdout.height {
            return None;
        }
        let mut out = DeepImage::new(self.width, self.height);
        out.samples
            .par_iter_mut()
            .zip(self.samples.par_iter())
            .zip(holdout.samples.par_iter())
            .for_each(|((dst, src), hold)| {
                for s in src {
                    let mut t = 1.0f32;
                    for h in hold {
                        if h.z < s.z {
                            t *= 1.0 - h.a;
                        }
                    }
                    if t > eps {
                        dst.push(s.scaled(t));
                    }
                }
                dst.sort_by(|a, b| a.z.partial_cmp(&b.z).unwrap_or(std::cmp::Ordering::Equal));
            });
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promote_and_demote() {
        let mut a = Image::new(2, 2, ChannelSet::Alpha);
        a.set(1, 1, 0, 0.5);
        assert_eq!(a.get_rgba(1, 1), [0.5; 4]);
        let rgb = Image::new(1, 1, ChannelSet::Rgb);
        assert_eq!(rgb.get_rgba(0, 0)[3], 1.0);
        let mut n = Image::new(1, 1, ChannelSet::Named(vec!["x".into(), "y".into()]));
        n.set_rgba(0, 0, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(n.get_rgba(0, 0), [1.0, 2.0, 0.0, 1.0]);
    }

    #[test]
    fn crop_resize_sample() {
        let img = Image::ramp(8, 1, true, [0.0; 4], [7.0, 0.0, 0.0, 1.0]);
        let c = img.crop(2, 0, 3, 1);
        assert_eq!(c.get_rgba(0, 0)[0], 2.0);
        assert_eq!(c.get_rgba(2, 0)[0], 4.0);
        let outside = img.crop(-1, 0, 2, 1);
        assert_eq!(outside.get_rgba(0, 0), [0.0; 4]);
        assert_eq!(outside.get_rgba(1, 0)[0], 0.0);
        assert!((img.sample_bilinear(2.5, 0.0)[0] - 2.5).abs() < 1e-6);
        assert!((img.sample_bicubic(3.0, 0.0)[0] - 3.0).abs() < 1e-5);
        let r = img.resize(4, 1);
        assert_eq!(r.width, 4);
        assert!(r.get_rgba(0, 0)[0] < r.get_rgba(3, 0)[0]);
    }

    #[test]
    fn display_encoding() {
        let img = Image::solid(1, 1, [0.0, 1.0, 0.5, 1.0]);
        let px = img.to_display_u8();
        assert_eq!(px, vec![0, 255, 188, 255]);
        assert!((srgb_decode(srgb_encode(0.3)) - 0.3).abs() < 1e-5);
    }

    #[test]
    fn multilayer_and_deep() {
        let mut ml = MultiLayerImage::new(2, 2);
        assert!(ml.insert("beauty", Image::rgba(2, 2)));
        assert!(!ml.insert("bad", Image::rgba(3, 2)));
        assert!(ml.beauty().is_some());
        let mut d = DeepImage::new(1, 1);
        d.push(0, 0, DeepSample::point(2.0, [0.0, 0.0, 1.0, 1.0]));
        d.push(0, 0, DeepSample::point(1.0, [0.5, 0.0, 0.0, 0.5]));
        let flat = d.flatten();
        assert_eq!(flat.get_rgba(0, 0), [0.5, 0.0, 0.5, 1.0]);
    }
}

/// A rectangular region of interest in pixel coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegionOfInterest {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl RegionOfInterest {
    /// Construct a region.
    pub fn new(x: usize, y: usize, width: usize, height: usize) -> Self {
        RegionOfInterest {
            x,
            y,
            width,
            height,
        }
    }
    /// Exclusive right edge.
    pub fn x_end(&self) -> usize {
        self.x + self.width
    }
    /// Exclusive bottom edge.
    pub fn y_end(&self) -> usize {
        self.y + self.height
    }
    /// True when a pixel lies inside.
    pub fn contains(&self, x: usize, y: usize) -> bool {
        x >= self.x && y >= self.y && x < self.x_end() && y < self.y_end()
    }
    /// Grow by `r` pixels on every side (clamped at zero).
    pub fn expanded(&self, r: usize) -> Self {
        let x = self.x.saturating_sub(r);
        let y = self.y.saturating_sub(r);
        RegionOfInterest {
            x,
            y,
            width: self.x_end() + r - x,
            height: self.y_end() + r - y,
        }
    }
    /// Shift by an offset (negative results clamp to zero and shrink).
    pub fn offset(&self, dx: i64, dy: i64) -> Self {
        let x0 = (self.x as i64 + dx).max(0);
        let y0 = (self.y as i64 + dy).max(0);
        let x1 = (self.x_end() as i64 + dx).max(x0);
        let y1 = (self.y_end() as i64 + dy).max(y0);
        RegionOfInterest {
            x: x0 as usize,
            y: y0 as usize,
            width: (x1 - x0) as usize,
            height: (y1 - y0) as usize,
        }
    }
    /// Intersection with an image of `w × h`.
    pub fn clipped(&self, w: usize, h: usize) -> Self {
        let x = self.x.min(w);
        let y = self.y.min(h);
        RegionOfInterest {
            x,
            y,
            width: self.x_end().min(w) - x,
            height: self.y_end().min(h) - y,
        }
    }
}
