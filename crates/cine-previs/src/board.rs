//! 2D storyboard: raster/vector layers, brushes, onion skin, light table
//! and the board itself (an ordered list of timed frames).
//!
//! Rasters are straight-alpha linear RGBA `f32` buffers. Stroke geometry is
//! in frame coordinates (`0..1`, origin top-left) so a board frame can be
//! shown over any viewport size; brush sizes are in pixels of the canvas.

use serde::{Deserialize, Serialize};
use ucg_core::EntityId;
use ucg_schema::CameraMove;

use crate::stroke::{Stroke, StrokePoint};

// ---------------------------------------------------------------------------
// Raster + brush
// ---------------------------------------------------------------------------

/// A round soft brush.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Brush {
    /// Radius in pixels at full pressure.
    pub radius_px: f32,
    /// `0` = fully soft (Gaussian-ish falloff from the centre), `1` = hard edge.
    pub hardness: f32,
    /// Linear RGBA colour (alpha multiplies opacity).
    pub color: [f32; 4],
    /// Overall opacity `0..1`.
    pub opacity: f32,
    /// Whether pressure scales the radius.
    pub pressure_size: bool,
    /// Whether pressure scales the opacity.
    pub pressure_opacity: bool,
}

impl Default for Brush {
    fn default() -> Self {
        Brush {
            radius_px: 6.0,
            hardness: 0.6,
            color: [0.0, 0.0, 0.0, 1.0],
            opacity: 1.0,
            pressure_size: true,
            pressure_opacity: true,
        }
    }
}

impl Brush {
    /// Coverage `0..1` of the brush at normalized distance `d = dist / radius`.
    pub fn falloff(&self, d: f32) -> f32 {
        if d >= 1.0 {
            return 0.0;
        }
        let h = self.hardness.clamp(0.0, 0.999);
        if d <= h {
            1.0
        } else {
            let t = (d - h) / (1.0 - h);
            1.0 - t * t * (3.0 - 2.0 * t)
        }
    }
}

/// A straight-alpha linear RGBA float image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Raster {
    /// Width in pixels.
    pub w: usize,
    /// Height in pixels.
    pub h: usize,
    /// `w × h × 4` interleaved RGBA.
    pub rgba: Vec<f32>,
}

impl Raster {
    /// A transparent raster.
    pub fn new(w: usize, h: usize) -> Self {
        Raster {
            w,
            h,
            rgba: vec![0.0; w * h * 4],
        }
    }
    /// A raster filled with one colour.
    pub fn filled(w: usize, h: usize, color: [f32; 4]) -> Self {
        let mut r = Raster::new(w, h);
        r.fill(color);
        r
    }
    /// Fill every pixel.
    pub fn fill(&mut self, color: [f32; 4]) {
        for (i, v) in self.rgba.iter_mut().enumerate() {
            *v = color[i % 4];
        }
    }
    /// Read a pixel (transparent black outside the image).
    pub fn pixel(&self, x: usize, y: usize) -> [f32; 4] {
        if x >= self.w || y >= self.h {
            return [0.0; 4];
        }
        let i = (y * self.w + x) * 4;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
    /// Write a pixel (ignored outside the image).
    pub fn set_pixel(&mut self, x: usize, y: usize, c: [f32; 4]) {
        if x < self.w && y < self.h {
            let i = (y * self.w + x) * 4;
            self.rgba[i..i + 4].copy_from_slice(&c);
        }
    }

    /// Composite `src` colour with coverage `a` over the pixel (straight alpha "over").
    fn blend_pixel(&mut self, x: usize, y: usize, src: [f32; 3], a: f32) {
        if a <= 0.0 || x >= self.w || y >= self.h {
            return;
        }
        let a = a.min(1.0);
        let dst = self.pixel(x, y);
        let out_a = a + dst[3] * (1.0 - a);
        let mut out = [0.0; 4];
        if out_a > 0.0 {
            for k in 0..3 {
                out[k] = (src[k] * a + dst[k] * dst[3] * (1.0 - a)) / out_a;
            }
        }
        out[3] = out_a;
        self.set_pixel(x, y, out);
    }

    /// Stamp a brush dab at pixel position `(x, y)` with the given pressure.
    pub fn stamp(&mut self, brush: &Brush, x: f32, y: f32, pressure: f32) {
        let pressure = pressure.clamp(0.0, 1.0);
        let radius = if brush.pressure_size {
            brush.radius_px * pressure
        } else {
            brush.radius_px
        };
        let alpha = brush.opacity
            * brush.color[3]
            * if brush.pressure_opacity {
                pressure
            } else {
                1.0
            };
        if radius <= 0.0 || alpha <= 0.0 {
            return;
        }
        let x0 = (x - radius).floor().max(0.0) as usize;
        let y0 = (y - radius).floor().max(0.0) as usize;
        let x1 = ((x + radius).ceil().max(0.0) as usize).min(self.w.saturating_sub(1));
        let y1 = ((y + radius).ceil().max(0.0) as usize).min(self.h.saturating_sub(1));
        if self.w == 0 || self.h == 0 {
            return;
        }
        let rgb = [brush.color[0], brush.color[1], brush.color[2]];
        for py in y0..=y1 {
            for px in x0..=x1 {
                let dx = px as f32 + 0.5 - x;
                let dy = py as f32 + 0.5 - y;
                let d = (dx * dx + dy * dy).sqrt() / radius;
                let cov = brush.falloff(d) * alpha;
                self.blend_pixel(px, py, rgb, cov);
            }
        }
    }

    /// Draw a stroke by stamping dabs along its polyline (spacing = radius / 2).
    pub fn draw_stroke(&mut self, brush: &Brush, stroke: &Stroke) {
        let (w, h) = (self.w as f32, self.h as f32);
        let to_px = |p: &StrokePoint| (p.x * w, p.y * h);
        if let [only] = stroke.points.as_slice() {
            let (x, y) = to_px(only);
            self.stamp(brush, x, y, only.pressure);
            return;
        }
        let spacing = (brush.radius_px * 0.5).max(0.5);
        for seg in stroke.points.windows(2) {
            let (ax, ay) = to_px(&seg[0]);
            let (bx, by) = to_px(&seg[1]);
            let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
            let steps = (len / spacing).ceil().max(1.0) as usize;
            for s in 0..=steps {
                let t = s as f32 / steps as f32;
                let p = seg[0].pressure + (seg[1].pressure - seg[0].pressure) * t;
                self.stamp(brush, ax + (bx - ax) * t, ay + (by - ay) * t, p);
            }
        }
    }

    /// Composite another raster over this one with a global opacity.
    pub fn blend_over(&mut self, src: &Raster, opacity: f32) {
        let opacity = opacity.clamp(0.0, 1.0);
        if opacity <= 0.0 {
            return;
        }
        for y in 0..self.h.min(src.h) {
            for x in 0..self.w.min(src.w) {
                let s = src.pixel(x, y);
                self.blend_pixel(x, y, [s[0], s[1], s[2]], s[3] * opacity);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Layers and canvas
// ---------------------------------------------------------------------------

/// A storyboard layer: vector strokes plus an optional raster.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    /// Display name.
    pub name: String,
    /// Hidden layers are skipped by [`composite`].
    pub visible: bool,
    /// Layer opacity `0..1`.
    pub opacity: f32,
    /// Vector strokes.
    pub strokes: Vec<Stroke>,
    /// Painted pixels.
    pub raster: Option<Raster>,
}

impl Layer {
    /// An empty, visible, opaque layer.
    pub fn new(name: impl Into<String>) -> Self {
        Layer {
            name: name.into(),
            visible: true,
            opacity: 1.0,
            strokes: Vec::new(),
            raster: None,
        }
    }
    /// Render the layer (raster, then strokes with `brush`) into a new raster.
    pub fn rasterize(&self, w: usize, h: usize, brush: &Brush) -> Raster {
        let mut out = Raster::new(w, h);
        if let Some(r) = &self.raster {
            out.blend_over(r, 1.0);
        }
        for s in &self.strokes {
            let b = Brush {
                color: s.color,
                radius_px: s.width_px * 0.5,
                ..*brush
            };
            out.draw_stroke(&b, s);
        }
        out
    }
}

/// Composite visible layers bottom-to-top into a `w × h` raster.
pub fn composite(layers: &[Layer], w: usize, h: usize) -> Raster {
    let brush = Brush::default();
    let mut out = Raster::new(w, h);
    for layer in layers.iter().filter(|l| l.visible && l.opacity > 0.0) {
        let r = if layer.strokes.is_empty() {
            match &layer.raster {
                Some(r) => r.clone(),
                None => continue,
            }
        } else {
            layer.rasterize(w, h, &brush)
        };
        out.blend_over(&r, layer.opacity);
    }
    out
}

/// A drawing surface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Canvas {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Layers bottom-to-top.
    pub layers: Vec<Layer>,
}

impl Canvas {
    /// A canvas with one empty layer named "Sketch".
    pub fn new(width: usize, height: usize) -> Self {
        Canvas {
            width,
            height,
            layers: vec![Layer::new("Sketch")],
        }
    }
    /// Width / height.
    pub fn aspect(&self) -> f64 {
        if self.height == 0 {
            0.0
        } else {
            self.width as f64 / self.height as f64
        }
    }
    /// Add a layer on top and return its index.
    pub fn add_layer(&mut self, name: impl Into<String>) -> usize {
        self.layers.push(Layer::new(name));
        self.layers.len() - 1
    }
    /// Append a stroke to `stroke.layer` (clamped to the top layer).
    pub fn add_stroke(&mut self, stroke: Stroke) {
        if self.layers.is_empty() {
            self.layers.push(Layer::new("Sketch"));
        }
        let idx = (stroke.layer as usize).min(self.layers.len() - 1);
        self.layers[idx].strokes.push(stroke);
    }
    /// Flatten every visible layer.
    pub fn composite(&self) -> Raster {
        composite(&self.layers, self.width, self.height)
    }
    /// All strokes on visible layers.
    pub fn strokes(&self) -> impl Iterator<Item = &Stroke> {
        self.layers
            .iter()
            .filter(|l| l.visible)
            .flat_map(|l| l.strokes.iter())
    }
}

// ---------------------------------------------------------------------------
// Vector helpers
// ---------------------------------------------------------------------------

/// A polyline vertex with a per-vertex width.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolyPoint {
    /// Frame x.
    pub x: f32,
    /// Frame y.
    pub y: f32,
    /// Width in pixels.
    pub width: f32,
}

/// A cubic Bézier segment in frame coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CubicBezier {
    /// Start point.
    pub p0: [f32; 2],
    /// First control point.
    pub p1: [f32; 2],
    /// Second control point.
    pub p2: [f32; 2],
    /// End point.
    pub p3: [f32; 2],
}

impl CubicBezier {
    /// Evaluate at `t ∈ 0..1`.
    pub fn at(&self, t: f32) -> [f32; 2] {
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        [
            a * self.p0[0] + b * self.p1[0] + c * self.p2[0] + d * self.p3[0],
            a * self.p0[1] + b * self.p1[1] + c * self.p2[1] + d * self.p3[1],
        ]
    }
}

impl Stroke {
    /// Convert to a polyline with widths from `pressure_to_width(pressure)`
    /// (the result is multiplied by `width_px`).
    pub fn to_polyline(&self, pressure_to_width: impl Fn(f32) -> f32) -> Vec<PolyPoint> {
        self.points
            .iter()
            .map(|p| PolyPoint {
                x: p.x,
                y: p.y,
                width: self.width_px * pressure_to_width(p.pressure),
            })
            .collect()
    }

    /// Convert to cubic Bézier segments through every sample (Catmull-Rom
    /// tangents, clamped ends). `n` points give `n - 1` segments.
    pub fn to_bezier(&self) -> Vec<CubicBezier> {
        let pts: Vec<[f32; 2]> = self.points.iter().map(|p| [p.x, p.y]).collect();
        let n = pts.len();
        if n < 2 {
            return Vec::new();
        }
        let get = |i: isize| pts[i.clamp(0, n as isize - 1) as usize];
        (0..n - 1)
            .map(|i| {
                let i = i as isize;
                let (pm, p0, p1, p2) = (get(i - 1), get(i), get(i + 1), get(i + 2));
                CubicBezier {
                    p0,
                    p1: [p0[0] + (p1[0] - pm[0]) / 6.0, p0[1] + (p1[1] - pm[1]) / 6.0],
                    p2: [p1[0] - (p2[0] - p0[0]) / 6.0, p1[1] - (p2[1] - p0[1]) / 6.0],
                    p3: p1,
                }
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Onion skin + light table
// ---------------------------------------------------------------------------

/// One ghosted neighbour frame produced by [`OnionSkin::frames`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct OnionFrame {
    /// Offset from the current frame (negative = before).
    pub frame_offset: i32,
    /// Ghost opacity.
    pub opacity: f32,
    /// Linear RGB tint (red-ish before, green-ish after).
    pub tint: [f32; 3],
}

/// Onion-skin settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct OnionSkin {
    /// Frames shown before the current one.
    pub before: u8,
    /// Frames shown after.
    pub after: u8,
    /// Opacity multiplier per step away (`opacity = falloff^distance`).
    pub opacity_falloff: f32,
}

impl Default for OnionSkin {
    fn default() -> Self {
        OnionSkin {
            before: 2,
            after: 1,
            opacity_falloff: 0.5,
        }
    }
}

/// Tint applied to frames before the current one.
pub const ONION_TINT_BEFORE: [f32; 3] = [1.0, 0.35, 0.35];
/// Tint applied to frames after the current one.
pub const ONION_TINT_AFTER: [f32; 3] = [0.35, 1.0, 0.45];

impl OnionSkin {
    /// Ghost frames ordered from the farthest "before" to the farthest "after".
    pub fn frames(&self) -> Vec<OnionFrame> {
        let f = self.opacity_falloff.clamp(0.0, 1.0);
        let mut out = Vec::with_capacity(self.before as usize + self.after as usize);
        for k in (1..=self.before as i32).rev() {
            out.push(OnionFrame {
                frame_offset: -k,
                opacity: f.powi(k),
                tint: ONION_TINT_BEFORE,
            });
        }
        for k in 1..=self.after as i32 {
            out.push(OnionFrame {
                frame_offset: k,
                opacity: f.powi(k),
                tint: ONION_TINT_AFTER,
            });
        }
        out
    }
}

/// A 2D similarity transform (translate in frame units, rotate in degrees).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform2D {
    /// Translation in frame units.
    pub translate: [f32; 2],
    /// Rotation about the frame centre in degrees.
    pub rotate_deg: f32,
    /// Scale about the frame centre.
    pub scale: [f32; 2],
}

impl Default for Transform2D {
    fn default() -> Self {
        Transform2D {
            translate: [0.0, 0.0],
            rotate_deg: 0.0,
            scale: [1.0, 1.0],
        }
    }
}

impl Transform2D {
    /// Apply to a frame point (scale and rotate about `(0.5, 0.5)`, then translate).
    pub fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        let (x, y) = ((p[0] - 0.5) * self.scale[0], (p[1] - 0.5) * self.scale[1]);
        let (s, c) = self.rotate_deg.to_radians().sin_cos();
        [
            x * c - y * s + 0.5 + self.translate[0],
            x * s + y * c + 0.5 + self.translate[1],
        ]
    }
}

/// A reference frame shown under the current drawing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LightTableRef {
    /// The referenced [`Frame::id`].
    pub frame_id: EntityId,
    /// Opacity of the reference.
    pub opacity: f32,
    /// Placement of the reference.
    pub transform: Transform2D,
}

/// The light table: arbitrary reference frames composited under the
/// current one (unlike onion skin, which is relative to the timeline).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct LightTable {
    /// References in stacking order.
    pub references: Vec<LightTableRef>,
}

impl LightTable {
    /// Add a reference.
    pub fn add(&mut self, frame_id: EntityId, opacity: f32, transform: Transform2D) {
        self.references.push(LightTableRef {
            frame_id,
            opacity,
            transform,
        });
    }
    /// Remove every reference to a frame.
    pub fn remove_frame(&mut self, frame_id: EntityId) {
        self.references.retain(|r| r.frame_id != frame_id);
    }
}

// ---------------------------------------------------------------------------
// Frames and board
// ---------------------------------------------------------------------------

/// One storyboard panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    /// Stable identity (shared with the graph's `board.frame` node).
    pub id: EntityId,
    /// How long the panel holds, in timeline frames.
    pub duration_frames: u32,
    /// The drawing.
    pub canvas: Canvas,
    /// Intended camera movement for the panel.
    pub camera_move_hint: CameraMove,
}

impl Frame {
    /// A one-second (24 fps) static panel on a 1920×1080 canvas.
    pub fn new() -> Self {
        Frame {
            id: EntityId::new(),
            duration_frames: 24,
            canvas: Canvas::new(1920, 1080),
            camera_move_hint: CameraMove::Static,
        }
    }
    /// A panel with an explicit canvas and duration.
    pub fn with_canvas(canvas: Canvas, duration_frames: u32) -> Self {
        Frame {
            id: EntityId::new(),
            duration_frames,
            canvas,
            camera_move_hint: CameraMove::Static,
        }
    }
}

impl Default for Frame {
    fn default() -> Self {
        Self::new()
    }
}

/// An ordered storyboard.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Board {
    /// Panels in order.
    pub frames: Vec<Frame>,
    /// Timeline rate the durations are counted in.
    pub fps: f64,
}

impl Default for Board {
    fn default() -> Self {
        Board {
            frames: Vec::new(),
            fps: 24.0,
        }
    }
}

impl Board {
    /// An empty 24 fps board.
    pub fn new() -> Self {
        Self::default()
    }
    /// Append a panel and return its index.
    pub fn push(&mut self, frame: Frame) -> usize {
        self.frames.push(frame);
        self.frames.len() - 1
    }
    /// Index of a panel by id.
    pub fn index_of(&self, id: EntityId) -> Option<usize> {
        self.frames.iter().position(|f| f.id == id)
    }
    /// Move the panel at `from` so it ends up at index `to`. Returns false
    /// if either index is out of range.
    pub fn reorder(&mut self, from: usize, to: usize) -> bool {
        if from >= self.frames.len() || to >= self.frames.len() {
            return false;
        }
        let f = self.frames.remove(from);
        self.frames.insert(to, f);
        true
    }
    /// Duplicate the panel at `index` right after itself (fresh id).
    /// Returns the new id.
    pub fn duplicate(&mut self, index: usize) -> Option<EntityId> {
        let src = self.frames.get(index)?;
        let mut copy = src.clone();
        copy.id = EntityId::new();
        let id = copy.id;
        self.frames.insert(index + 1, copy);
        Some(id)
    }
    /// Remove a panel.
    pub fn remove(&mut self, index: usize) -> Option<Frame> {
        if index < self.frames.len() {
            Some(self.frames.remove(index))
        } else {
            None
        }
    }
    /// Total duration in timeline frames.
    pub fn total_frames(&self) -> u64 {
        self.frames.iter().map(|f| f.duration_frames as u64).sum()
    }
    /// Total duration in seconds.
    pub fn total_seconds(&self) -> f64 {
        if self.fps <= 0.0 {
            0.0
        } else {
            self.total_frames() as f64 / self.fps
        }
    }
    /// Which panel is showing at timeline frame `t`, with the offset into it.
    pub fn frame_at(&self, t: u64) -> Option<(usize, u64)> {
        let mut start = 0u64;
        for (i, f) in self.frames.iter().enumerate() {
            let end = start + f.duration_frames as u64;
            if t < end {
                return Some((i, t - start));
            }
            start = end;
        }
        None
    }
    /// Timeline start frame of each panel.
    pub fn start_frames(&self) -> Vec<u64> {
        let mut out = Vec::with_capacity(self.frames.len());
        let mut acc = 0u64;
        for f in &self.frames {
            out.push(acc);
            acc += f.duration_frames as u64;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brush_falloff_shape() {
        let b = Brush {
            hardness: 0.5,
            ..Default::default()
        };
        assert_eq!(b.falloff(0.0), 1.0);
        assert_eq!(b.falloff(0.5), 1.0);
        assert!(b.falloff(0.75) > 0.0 && b.falloff(0.75) < 1.0);
        assert_eq!(b.falloff(1.0), 0.0);
    }

    #[test]
    fn bezier_passes_through_samples() {
        let s = Stroke::from_points(vec![
            StrokePoint::new(0.0, 0.0),
            StrokePoint::new(0.5, 0.5),
            StrokePoint::new(1.0, 0.0),
        ]);
        let b = s.to_bezier();
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].at(0.0), [0.0, 0.0]);
        assert_eq!(b[0].at(1.0), [0.5, 0.5]);
        assert_eq!(b[1].p3, [1.0, 0.0]);
        let pl = s.to_polyline(|p| p);
        assert_eq!(pl.len(), 3);
        assert_eq!(pl[0].width, 4.0);
    }

    #[test]
    fn transform2d_identity_and_translate() {
        let t = Transform2D::default();
        let p = t.apply([0.2, 0.7]);
        assert!((p[0] - 0.2).abs() < 1e-6 && (p[1] - 0.7).abs() < 1e-6);
        let t = Transform2D {
            translate: [0.1, 0.0],
            ..Default::default()
        };
        let p = t.apply([0.5, 0.5]);
        assert!((p[0] - 0.6).abs() < 1e-6 && (p[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn board_timing() {
        let mut b = Board::new();
        let mut f1 = Frame::new();
        f1.duration_frames = 10;
        let mut f2 = Frame::new();
        f2.duration_frames = 20;
        b.push(f1);
        b.push(f2);
        assert_eq!(b.total_frames(), 30);
        assert_eq!(b.frame_at(15), Some((1, 5)));
        assert_eq!(b.frame_at(30), None);
        assert_eq!(b.start_frames(), vec![0, 10]);
    }
}
