//! Previs render-pass providers: how `Read(PrevisPass)` nodes obtain pixels.
//!
//! The compositor never renders 3D itself; the previs stage (or a cache)
//! implements [`PassProvider`]. [`SyntheticPassProvider`] renders an analytic
//! shaded sphere with depth, normals, ids and motion so tests and demos are
//! self-contained.

use crate::graph_model::PassKind;
use crate::image::{ChannelSet, Image, MultiLayerImage, RegionOfInterest};
use ucg_core::EntityId;

/// Source of previs render passes for the evaluator.
pub trait PassProvider: Send + Sync {
    /// A full-frame pass, or `None` if unavailable.
    fn pass(&self, scene: EntityId, camera: EntityId, pass: PassKind, frame: i64) -> Option<Image>;

    /// Same as [`PassProvider::pass`] but only `roi` needs valid pixels
    /// (the rest may be zero). The default ignores the region.
    fn pass_roi(
        &self,
        scene: EntityId,
        camera: EntityId,
        pass: PassKind,
        frame: i64,
        _roi: Option<RegionOfInterest>,
    ) -> Option<Image> {
        self.pass(scene, camera, pass, frame)
    }
}

/// Provider that has no passes at all.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullPassProvider;

impl PassProvider for NullPassProvider {
    fn pass(&self, _: EntityId, _: EntityId, _: PassKind, _: i64) -> Option<Image> {
        None
    }
}

/// Analytic test renderer: a lit sphere drifting across a checkered
/// background, with matching depth / normal / id / motion passes.
#[derive(Clone, Debug, PartialEq)]
pub struct SyntheticPassProvider {
    pub width: usize,
    pub height: usize,
    /// Sphere radius as a fraction of the image height.
    pub sphere_radius: f32,
    /// Camera-space distance to the sphere center.
    pub sphere_distance: f32,
    /// World-space sphere radius (depth varies by ± this across the sphere).
    pub sphere_world_radius: f32,
    /// Depth of the background plane.
    pub background_depth: f32,
    /// Horizontal drift of the sphere per frame, in pixels.
    pub drift_per_frame: f32,
    /// Sphere albedo (linear RGB).
    pub albedo: [f32; 3],
    /// Light direction (camera space, +z towards the camera).
    pub light_dir: [f32; 3],
    /// Background checker colors and square size.
    pub background: [[f32; 3]; 2],
    pub checker_size: usize,
    /// If set, only this scene is served.
    pub scene: Option<EntityId>,
}

impl Default for SyntheticPassProvider {
    fn default() -> Self {
        SyntheticPassProvider {
            width: 128,
            height: 96,
            sphere_radius: 0.3,
            sphere_distance: 10.0,
            sphere_world_radius: 2.0,
            background_depth: 40.0,
            drift_per_frame: 1.0,
            albedo: [0.8, 0.3, 0.2],
            light_dir: [0.4, 0.6, 0.7],
            background: [[0.18, 0.18, 0.2], [0.28, 0.28, 0.3]],
            checker_size: 8,
            scene: None,
        }
    }
}

impl SyntheticPassProvider {
    /// Provider rendering `width × height` passes.
    pub fn new(width: usize, height: usize) -> Self {
        SyntheticPassProvider {
            width,
            height,
            ..Default::default()
        }
    }

    /// Sphere center in pixels at a frame.
    pub fn sphere_center(&self, frame: i64) -> [f32; 2] {
        [
            self.width as f32 / 2.0 + frame as f32 * self.drift_per_frame,
            self.height as f32 / 2.0,
        ]
    }

    /// Sphere radius in pixels.
    pub fn sphere_radius_px(&self) -> f32 {
        self.sphere_radius * self.height as f32
    }

    /// Depth of the sphere surface at its center (the natural focus distance).
    pub fn sphere_front_depth(&self) -> f32 {
        self.sphere_distance - self.sphere_world_radius
    }

    /// Whether a pixel is covered by the sphere at a frame.
    pub fn covers(&self, x: usize, y: usize, frame: i64) -> bool {
        let c = self.sphere_center(frame);
        let r = self.sphere_radius_px();
        let dx = (x as f32 + 0.5 - c[0]) / r;
        let dy = (y as f32 + 0.5 - c[1]) / r;
        dx * dx + dy * dy <= 1.0
    }

    /// Render every pass at once.
    pub fn render_all(&self, frame: i64) -> MultiLayerImage {
        let mut ml = MultiLayerImage::new(self.width, self.height);
        for pass in PassKind::ALL {
            ml.insert(pass.as_str(), self.render(*pass, frame, None));
        }
        ml
    }

    /// Render one pass; with `roi`, only that region, as a windowed image.
    pub fn render(&self, pass: PassKind, frame: i64, roi: Option<RegionOfInterest>) -> Image {
        let channels = match pass {
            PassKind::Beauty => ChannelSet::Rgba,
            PassKind::Depth => ChannelSet::Depth,
            PassKind::Normal => ChannelSet::Normal,
            PassKind::Id => ChannelSet::Named(vec!["id".to_string()]),
            PassKind::Motion => ChannelSet::Motion,
        };
        let roi = roi
            .map(|r| r.clipped(self.width, self.height))
            .unwrap_or(RegionOfInterest::new(0, 0, self.width, self.height));
        // Only the region is allocated, placed where it is on the frame.
        let mut img =
            Image::new(roi.width, roi.height, channels).placed(roi.x as i64, roi.y as i64);
        let c = self.sphere_center(frame);
        let r = self.sphere_radius_px();
        let l = {
            let [x, y, z] = self.light_dir;
            let n = (x * x + y * y + z * z).sqrt().max(1e-6);
            [x / n, y / n, z / n]
        };
        let cs = self.checker_size.max(1);
        for y in roi.y..roi.y_end() {
            for x in roi.x..roi.x_end() {
                let dx = (x as f32 + 0.5 - c[0]) / r;
                let dy = (y as f32 + 0.5 - c[1]) / r;
                let d2 = dx * dx + dy * dy;
                if d2 <= 1.0 {
                    let nz = (1.0 - d2).sqrt();
                    let n = [dx, -dy, nz];
                    match pass {
                        PassKind::Beauty => {
                            let ndl = (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).max(0.0);
                            let k = 0.15 + 0.85 * ndl;
                            img.set_pixel(
                                x - roi.x,
                                y - roi.y,
                                &[
                                    self.albedo[0] * k,
                                    self.albedo[1] * k,
                                    self.albedo[2] * k,
                                    1.0,
                                ],
                            );
                        }
                        PassKind::Depth => img.set_pixel(
                            x - roi.x,
                            y - roi.y,
                            &[self.sphere_distance - nz * self.sphere_world_radius],
                        ),
                        PassKind::Normal => img.set_pixel(x - roi.x, y - roi.y, &n),
                        PassKind::Id => img.set_pixel(x - roi.x, y - roi.y, &[1.0]),
                        PassKind::Motion => {
                            img.set_pixel(x - roi.x, y - roi.y, &[self.drift_per_frame, 0.0])
                        }
                    }
                } else {
                    match pass {
                        PassKind::Beauty => {
                            let cc = self.background[((x / cs) + (y / cs)) % 2];
                            img.set_pixel(x - roi.x, y - roi.y, &[cc[0], cc[1], cc[2], 1.0]);
                        }
                        PassKind::Depth => {
                            img.set_pixel(x - roi.x, y - roi.y, &[self.background_depth])
                        }
                        PassKind::Normal => img.set_pixel(x - roi.x, y - roi.y, &[0.0, 0.0, 1.0]),
                        PassKind::Id => img.set_pixel(x - roi.x, y - roi.y, &[0.0]),
                        PassKind::Motion => img.set_pixel(x - roi.x, y - roi.y, &[0.0, 0.0]),
                    }
                }
            }
        }
        img
    }
}

impl PassProvider for SyntheticPassProvider {
    fn pass(&self, scene: EntityId, camera: EntityId, pass: PassKind, frame: i64) -> Option<Image> {
        self.pass_roi(scene, camera, pass, frame, None)
    }

    fn pass_roi(
        &self,
        scene: EntityId,
        _camera: EntityId,
        pass: PassKind,
        frame: i64,
        roi: Option<RegionOfInterest>,
    ) -> Option<Image> {
        if let Some(only) = self.scene {
            if only != scene {
                return None;
            }
        }
        Some(self.render(pass, frame, roi))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sphere_passes_are_consistent() {
        let p = SyntheticPassProvider::new(64, 48);
        let ml = p.render_all(0);
        let depth = ml.depth().unwrap();
        let beauty = ml.beauty().unwrap();
        let id = ml.id().unwrap();
        let (cx, cy) = (32, 24);
        assert!(p.covers(cx, cy, 0));
        assert!((depth.get(cx, cy, 0) - p.sphere_front_depth()).abs() < 1e-2);
        assert_eq!(depth.get(0, 0, 0), p.background_depth);
        assert_eq!(id.get(cx, cy, 0), 1.0);
        assert_eq!(id.get(0, 0, 0), 0.0);
        assert!(beauty.get_alpha(cx, cy) == 1.0);
        let n = ml.normal().unwrap().pixel(cx, cy);
        assert!((n[2] - 1.0).abs() < 0.05);
        // drift moves the sphere
        assert_ne!(
            p.render(PassKind::Id, 0, None),
            p.render(PassKind::Id, 10, None)
        );
        // roi allocates only the region, placed on the frame
        let part = p.render(PassKind::Depth, 0, Some(RegionOfInterest::new(2, 3, 4, 4)));
        assert_eq!((part.width, part.height, part.x, part.y), (4, 4, 2, 3));
        assert_eq!(part.canvas_rgba(2, 3)[0], p.background_depth);
        assert_eq!(part.canvas_rgba(10, 10), [0.0; 4]);
        // scene filter
        let only = SyntheticPassProvider {
            scene: Some(EntityId::new()),
            ..Default::default()
        };
        assert!(only
            .pass(EntityId::new(), EntityId::nil(), PassKind::Beauty, 0)
            .is_none());
        assert!(NullPassProvider
            .pass(EntityId::nil(), EntityId::nil(), PassKind::Beauty, 0)
            .is_none());
    }
}
