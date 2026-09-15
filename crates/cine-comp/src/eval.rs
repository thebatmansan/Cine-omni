//! Lazy, cached, parallel CPU evaluation of a [`CompGraph`].
//!
//! `evaluate(graph, node, frame)` pulls the requested node: every upstream
//! node is evaluated on demand (once), independent inputs of a node are
//! evaluated in parallel with rayon, and results are cached under
//! `(node, frame, hash)` where the hash covers the node's own parameters and
//! everything upstream. Changing a parameter therefore invalidates exactly
//! that node and its downstream; untouched branches keep hitting the cache.
//!
//! # Region of interest
//!
//! [`Evaluator::evaluate_roi`] threads a [`RegionOfInterest`] up the pull
//! chain. What is actually lazy in this CPU reference implementation:
//! * `Crop` requests only its own rectangle from upstream;
//! * `Read(PrevisPass)` forwards the region to the [`PassProvider`], which may
//!   compute only those pixels (the shipped [`SyntheticPassProvider`] does);
//! * filters expand the region by their kernel radius; point operations pass
//!   it through unchanged; `Transform`, `Reformat`, `Roto` and deep nodes
//!   request the full frame.
//!
//! Results carry a data window ([`Image`]): a node evaluated for a region
//! answers with a buffer the size of that region, placed on the canvas, and
//! generators and previs passes allocate only the region. Nodes that request
//! the full frame from upstream still compute it, and their result is cropped
//! to the region before it is cached or passed on. The region is part of the
//! cache key.

use crate::error::{Error, Result};
use crate::graph_model::{
    BlurKind, CompGraph, CompNode, CopyChannels, FilterKind, KeyerKind, MergeOp, NodeId, NodeKind,
    PassKind, ProceduralKind, ReadSource, ShuffleSource,
};
use crate::image::{DeepImage, Image, RegionOfInterest};
use crate::ops;
use crate::provider::{NullPassProvider, PassProvider, SyntheticPassProvider};
use rayon::prelude::*;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

/// Result of evaluating a node: a flat or a deep image.
#[derive(Clone, Debug)]
pub enum Output {
    Image(Arc<Image>),
    Deep(Arc<DeepImage>),
}

impl Output {
    /// The flat image, or an error if the output is deep.
    pub fn image(&self, node: &str) -> Result<Arc<Image>> {
        match self {
            Output::Image(i) => Ok(i.clone()),
            Output::Deep(_) => Err(Error::OutputMismatch {
                node: node.into(),
                expected: "image",
                got: "deep",
            }),
        }
    }
    /// The deep image, or an error if the output is flat.
    pub fn deep(&self, node: &str) -> Result<Arc<DeepImage>> {
        match self {
            Output::Deep(d) => Ok(d.clone()),
            Output::Image(_) => Err(Error::OutputMismatch {
                node: node.into(),
                expected: "deep",
                got: "image",
            }),
        }
    }
}

/// Counters kept by the evaluator.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EvalStats {
    /// Node executions (cache misses).
    pub nodes_evaluated: usize,
    /// Results served from the cache.
    pub cache_hits: usize,
    /// Nodes executed since the last reset, in completion order.
    pub evaluated: Vec<NodeId>,
}

/// One `Write` node output captured during evaluation.
#[derive(Clone, Debug)]
pub struct WriteRecord {
    pub node: NodeId,
    pub path: String,
    pub frame: i64,
    pub image: Arc<Image>,
}

type CacheKey = (NodeId, i64, u64);

/// Caching graph evaluator. Safe to share between threads.
pub struct Evaluator {
    cache: Mutex<HashMap<CacheKey, Output>>,
    provider: Box<dyn PassProvider>,
    files: HashMap<String, Arc<Image>>,
    stats: Mutex<EvalStats>,
    writes: Mutex<Vec<WriteRecord>>,
    /// Evaluate independent inputs in parallel (default true).
    pub parallel: bool,
}

struct Ctx<'a> {
    graph: &'a CompGraph,
    frame: i64,
    hashes: HashMap<NodeId, u64>,
}

impl Default for Evaluator {
    fn default() -> Self {
        Evaluator::new(Box::new(NullPassProvider))
    }
}

impl Evaluator {
    /// Evaluator pulling previs passes from `provider`.
    pub fn new(provider: Box<dyn PassProvider>) -> Self {
        Evaluator {
            cache: Mutex::new(HashMap::new()),
            provider,
            files: HashMap::new(),
            stats: Mutex::new(EvalStats::default()),
            writes: Mutex::new(Vec::new()),
            parallel: true,
        }
    }

    /// Evaluator backed by the default [`SyntheticPassProvider`].
    pub fn with_synthetic() -> Self {
        Evaluator::new(Box::new(SyntheticPassProvider::default()))
    }

    /// Register an in-memory image for `Read(File { path })` nodes.
    pub fn register_file(&mut self, path: impl Into<String>, image: Image) {
        self.files.insert(path.into(), Arc::new(image));
    }

    /// Evaluate `node` at `frame` as a flat image.
    pub fn evaluate(&self, graph: &CompGraph, node: NodeId, frame: i64) -> Result<Arc<Image>> {
        let name = graph.node(node)?.name.clone();
        self.evaluate_output(graph, node, frame, None)?.image(&name)
    }

    /// Evaluate `node` at `frame`, computing only `roi` where possible.
    pub fn evaluate_roi(
        &self,
        graph: &CompGraph,
        node: NodeId,
        frame: i64,
        roi: RegionOfInterest,
    ) -> Result<Arc<Image>> {
        let name = graph.node(node)?.name.clone();
        self.evaluate_output(graph, node, frame, Some(roi))?
            .image(&name)
    }

    /// Evaluate `node` at `frame` as a deep image.
    pub fn evaluate_deep(
        &self,
        graph: &CompGraph,
        node: NodeId,
        frame: i64,
    ) -> Result<Arc<DeepImage>> {
        let name = graph.node(node)?.name.clone();
        self.evaluate_output(graph, node, frame, None)?.deep(&name)
    }

    /// Evaluate a node to either kind of output.
    pub fn evaluate_output(
        &self,
        graph: &CompGraph,
        node: NodeId,
        frame: i64,
        roi: Option<RegionOfInterest>,
    ) -> Result<Output> {
        graph.node(node)?;
        let ctx = Ctx {
            graph,
            frame,
            hashes: graph.hashes()?,
        };
        self.eval_node(&ctx, node, roi)
    }

    /// Drop every cached result of a node (all frames / regions).
    pub fn invalidate(&self, node: NodeId) {
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|k, _| k.0 != node);
    }

    /// Drop the whole cache.
    pub fn clear_cache(&self) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// Number of cached results.
    pub fn cache_len(&self) -> usize {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Counters since the last reset.
    pub fn stats(&self) -> EvalStats {
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Reset the counters.
    pub fn reset_stats(&self) {
        *self.stats.lock().unwrap_or_else(|e| e.into_inner()) = EvalStats::default();
    }

    /// Outputs recorded by `Write` nodes (cleared by [`Evaluator::take_writes`]).
    pub fn writes(&self) -> Vec<WriteRecord> {
        self.writes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Take and clear the recorded writes.
    pub fn take_writes(&self) -> Vec<WriteRecord> {
        std::mem::take(&mut *self.writes.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn cache_key(ctx: &Ctx, id: NodeId, roi: Option<RegionOfInterest>) -> CacheKey {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        ctx.hashes.get(&id).copied().unwrap_or(0).hash(&mut h);
        roi.hash(&mut h);
        (id, ctx.frame, h.finish())
    }

    fn eval_node(&self, ctx: &Ctx, id: NodeId, roi: Option<RegionOfInterest>) -> Result<Output> {
        let node = ctx.graph.node(id)?;
        let key = Self::cache_key(ctx, id, roi);
        if let Some(hit) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned()
        {
            self.stats
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .cache_hits += 1;
            return Ok(hit);
        }
        let links = ctx.graph.inputs(id)?;
        let wanted: Vec<(usize, NodeId)> = links
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.map(|(n, _)| (i, n)))
            .collect();
        let results: Vec<Result<(usize, Output)>> = if self.parallel && wanted.len() > 1 {
            wanted
                .par_iter()
                .map(|(port, src)| {
                    self.eval_node(ctx, *src, input_roi(node, *port, roi))
                        .map(|o| (*port, o))
                })
                .collect()
        } else {
            wanted
                .iter()
                .map(|(port, src)| {
                    self.eval_node(ctx, *src, input_roi(node, *port, roi))
                        .map(|o| (*port, o))
                })
                .collect()
        };
        let mut inputs: Vec<Option<Output>> = vec![None; links.len()];
        for r in results {
            let (port, out) = r?;
            inputs[port] = Some(out);
        }
        let out = if node.bypass && inputs.first().is_some_and(Option::is_some) {
            inputs[0].clone().ok_or(Error::NodeNotFound(id))?
        } else {
            self.execute(node, &inputs, ctx.frame, roi)?
        };
        // A node asked for a region answers with that region. Inputs may have
        // been widened (a blur's kernel margin) and some nodes still compute
        // the full frame; neither leaks out past the region, into the cache or
        // downstream.
        let out = match (out, roi) {
            (Output::Image(img), Some(r)) => {
                let (x, y, w, h) = img.window();
                let (rx, ry) = (r.x as i64, r.y as i64);
                // Intersect with what exists: a region past the edge of the
                // frame is not invented.
                let x0 = rx.max(x);
                let y0 = ry.max(y);
                let x1 = (rx + r.width as i64).min(x + w as i64).max(x0);
                let y1 = (ry + r.height as i64).min(y + h as i64).max(y0);
                if (x0, y0, x1 - x0, y1 - y0) == (x, y, w as i64, h as i64) {
                    Output::Image(img)
                } else {
                    Output::Image(Arc::new(img.crop_canvas(
                        x0,
                        y0,
                        (x1 - x0) as usize,
                        (y1 - y0) as usize,
                    )))
                }
            }
            (other, _) => other,
        };
        {
            let mut s = self.stats.lock().unwrap_or_else(|e| e.into_inner());
            s.nodes_evaluated += 1;
            s.evaluated.push(id);
        }
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, out.clone());
        Ok(out)
    }

    fn execute(
        &self,
        node: &CompNode,
        inputs: &[Option<Output>],
        frame: i64,
        roi: Option<RegionOfInterest>,
    ) -> Result<Output> {
        let desc = node.kind.describe();
        let img = |port: usize| -> Result<Arc<Image>> {
            match inputs.get(port).and_then(Option::as_ref) {
                Some(o) => o.image(&node.name),
                None => Err(Error::MissingInput {
                    node: node.name.clone(),
                    port: desc
                        .inputs
                        .get(port)
                        .map(|p| p.name.clone())
                        .unwrap_or_default(),
                }),
            }
        };
        let opt_img = |port: usize| -> Result<Option<Arc<Image>>> {
            match inputs.get(port).and_then(Option::as_ref) {
                Some(o) => o.image(&node.name).map(Some),
                None => Ok(None),
            }
        };
        let deep = |port: usize| -> Result<Arc<DeepImage>> {
            match inputs.get(port).and_then(Option::as_ref) {
                Some(o) => o.deep(&node.name),
                None => Err(Error::MissingInput {
                    node: node.name.clone(),
                    port: desc
                        .inputs
                        .get(port)
                        .map(|p| p.name.clone())
                        .unwrap_or_default(),
                }),
            }
        };
        let bad = |param: &str, reason: &str| Error::BadParam {
            node: node.name.clone(),
            param: param.into(),
            reason: reason.into(),
        };
        let flat = |i: Image| Ok(Output::Image(Arc::new(i)));

        match node.kind {
            NodeKind::Read => match node.read_source() {
                ReadSource::File { path } => self
                    .files
                    .get(&path)
                    .cloned()
                    .map(Output::Image)
                    .ok_or_else(|| Error::MissingSource(format!("file `{path}` not registered"))),
                ReadSource::Generator {
                    generator,
                    width,
                    height,
                    color,
                } => {
                    use crate::graph_model::GeneratorKind::*;
                    // Generated over the region only: a generator is a function
                    // of the pixel, so nothing outside the region is computed or
                    // allocated. Same values, pixel for pixel, as the full frame.
                    let black = [0.0, 0.0, 0.0, 1.0];
                    let img = match generator {
                        Checkerboard => Image::generate_window(width, height, roi, |x, y| {
                            if ((x / 16) + (y / 16)).is_multiple_of(2) {
                                color
                            } else {
                                black
                            }
                        }),
                        Ramp => Image::generate_window(width, height, roi, |x, _| {
                            let t = if width > 1 {
                                x as f32 / (width - 1) as f32
                            } else {
                                0.0
                            };
                            let mut c = [0.0; 4];
                            for (i, v) in c.iter_mut().enumerate() {
                                *v = black[i] + (color[i] - black[i]) * t;
                            }
                            c
                        }),
                        Solid => Image::generate_window(width, height, roi, |_, _| color),
                        Black => Image::generate_window(width, height, roi, |_, _| [0.0; 4]),
                    };
                    flat(img)
                }
                ReadSource::PrevisPass {
                    scene,
                    pass,
                    camera,
                } => self
                    .provider
                    .pass_roi(scene, camera, pass, frame, roi)
                    .map(|i| Output::Image(Arc::new(i)))
                    .ok_or_else(|| {
                        Error::MissingSource(format!(
                            "previs pass {} of scene {scene} (camera {camera})",
                            pass.as_str()
                        ))
                    }),
            },
            NodeKind::Merge => {
                let op = node
                    .param("operation")
                    .and_then(MergeOp::from_param)
                    .ok_or_else(|| bad("operation", "unknown"))?;
                let a = img(0)?;
                let b = img(1)?;
                let mask = opt_img(2)?;
                flat(ops::merge(op, &a, &b, mask.as_deref(), node.f32("mix")))
            }
            NodeKind::Keyer => {
                let kind = node
                    .param("kind")
                    .and_then(KeyerKind::from_param)
                    .ok_or_else(|| bad("kind", "unknown"))?;
                let src = img(0)?;
                let invert = node.bool("invert");
                let out = match kind {
                    KeyerKind::Chroma => ops::chroma_key(
                        &src,
                        &ops::ChromaKey {
                            key_color: node.color("key_color"),
                            tolerance: node.f32("tolerance"),
                            softness: node.f32("softness"),
                            spill_suppress: node.f32("spill_suppress"),
                            invert,
                        },
                    ),
                    KeyerKind::Luma => {
                        ops::luma_key(&src, node.f32("low"), node.f32("high"), invert)
                    }
                    KeyerKind::Difference => {
                        let plate = img(1)?;
                        ops::difference_key(&src, &plate, node.f32("threshold"), invert)
                    }
                    KeyerKind::Screen => ops::screen_key(
                        &src,
                        node.color("key_color"),
                        node.f32("spill_suppress"),
                        invert,
                    ),
                };
                flat(out)
            }
            NodeKind::ColorCorrect | NodeKind::Grade => {
                let cc = ops::ColorCorrect {
                    gain: node.color("gain"),
                    gamma: node.f32("gamma"),
                    lift: node.color("lift"),
                    offset: node.color("offset"),
                    saturation: node.f32("saturation"),
                    contrast: node.f32("contrast"),
                };
                let src = img(0)?;
                let mask = opt_img(1)?;
                if cc.is_identity() && mask.is_none() {
                    return Ok(Output::Image(src));
                }
                flat(ops::color_correct(&src, &cc, mask.as_deref()))
            }
            NodeKind::Transform => {
                let t = ops::Transform2D {
                    translate: node.vec2("translate"),
                    rotate_deg: node.f32("rotate_deg"),
                    scale: node.vec2("scale"),
                    center: if node.bool("center_auto") {
                        None
                    } else {
                        Some(node.vec2("center"))
                    },
                    filter: node
                        .param("filter")
                        .and_then(FilterKind::from_param)
                        .unwrap_or(FilterKind::Bilinear),
                };
                flat(ops::transform(&*img(0)?, &t))
            }
            NodeKind::Crop => {
                let w = usize::try_from(node.int("width")).map_err(|_| bad("width", "negative"))?;
                let h =
                    usize::try_from(node.int("height")).map_err(|_| bad("height", "negative"))?;
                flat(ops::crop(&*img(0)?, node.int("x"), node.int("y"), w, h))
            }
            NodeKind::Reformat => {
                let w = usize::try_from(node.int("width")).map_err(|_| bad("width", "negative"))?;
                let h =
                    usize::try_from(node.int("height")).map_err(|_| bad("height", "negative"))?;
                let filter = node
                    .param("filter")
                    .and_then(FilterKind::from_param)
                    .unwrap_or(FilterKind::Bilinear);
                flat(ops::reformat(&*img(0)?, w, h, filter))
            }
            NodeKind::Blur => {
                let kind = node
                    .param("kind")
                    .and_then(BlurKind::from_param)
                    .unwrap_or(BlurKind::Gaussian);
                let src = img(0)?;
                let mask = opt_img(1)?;
                flat(ops::blur_masked(
                    &src,
                    node.f32("size"),
                    kind,
                    mask.as_deref(),
                ))
            }
            NodeKind::Sharpen => flat(ops::sharpen(
                &*img(0)?,
                node.f32("amount"),
                node.f32("size"),
            )),
            NodeKind::Erode => flat(ops::erode_alpha(
                &*img(0)?,
                node.int("size").max(0) as usize,
            )),
            NodeKind::Dilate => flat(ops::dilate_alpha(
                &*img(0)?,
                node.int("size").max(0) as usize,
            )),
            NodeKind::EdgeBlur => flat(ops::edge_blur(
                &*img(0)?,
                node.f32("size"),
                node.int("edge_width").max(0) as usize,
            )),
            NodeKind::Roto => {
                let (w, h) = match opt_img(0)? {
                    Some(src) => (src.width, src.height),
                    None => (
                        node.int("width").max(0) as usize,
                        node.int("height").max(0) as usize,
                    ),
                };
                flat(ops::rasterize(
                    &node.shapes("shapes"),
                    w,
                    h,
                    node.bool("invert"),
                ))
            }
            NodeKind::ZDefocus => {
                let p = ops::ZDefocus {
                    focal_distance: node.f32("focal_distance"),
                    f_stop: node.f32("f_stop"),
                    max_blur: node.f32("max_blur"),
                    slices: node.int("slices").max(2) as usize,
                };
                let src = img(0)?;
                let depth = img(1)?;
                flat(ops::z_defocus(&src, &depth, &p))
            }
            NodeKind::DeepMerge => Ok(Output::Deep(Arc::new(ops::deep_merge(
                &*deep(0)?,
                &*deep(1)?,
            )?))),
            NodeKind::DeepToImage => flat(ops::deep_to_image(&*deep(0)?)),
            NodeKind::DeepFromImage => Ok(Output::Deep(Arc::new(ops::deep_from_image(
                &*img(0)?,
                &*img(1)?,
            )?))),
            NodeKind::DeepHoldout => Ok(Output::Deep(Arc::new(ops::deep_holdout(
                &*deep(0)?,
                &*deep(1)?,
                node.f32("eps"),
            )?))),
            NodeKind::Copy => {
                let which = node
                    .param("channels")
                    .and_then(CopyChannels::from_param)
                    .unwrap_or(CopyChannels::Alpha);
                flat(ops::copy_channels(&*img(0)?, &*img(1)?, which))
            }
            NodeKind::Shuffle => {
                let pick = |n: &str, d: ShuffleSource| {
                    node.param(n)
                        .and_then(ShuffleSource::from_param)
                        .unwrap_or(d)
                };
                let sources = [
                    pick("r", ShuffleSource::R),
                    pick("g", ShuffleSource::G),
                    pick("b", ShuffleSource::B),
                    pick("a", ShuffleSource::A),
                ];
                flat(ops::shuffle(&*img(0)?, sources))
            }
            NodeKind::Premult => flat(ops::premult(&*img(0)?)),
            NodeKind::Unpremult => flat(ops::unpremult(&*img(0)?)),
            NodeKind::Invert => flat(ops::invert(&*img(0)?)),
            NodeKind::Clamp => flat(ops::clamp(
                &*img(0)?,
                node.f32("min"),
                node.f32("max"),
                node.bool("clamp_alpha"),
            )),
            NodeKind::Expression => {
                let (r, g, b, a) = (node.str("r"), node.str("g"), node.str("b"), node.str("a"));
                flat(ops::expression(
                    &*img(0)?,
                    [Some(&r), Some(&g), Some(&b), Some(&a)],
                )?)
            }
            NodeKind::Procedural => {
                let p = ops::Procedural {
                    kind: node
                        .param("kind")
                        .and_then(ProceduralKind::from_param)
                        .unwrap_or(ProceduralKind::Noise),
                    width: node.int("width").max(0) as usize,
                    height: node.int("height").max(0) as usize,
                    seed: node.int("seed") as u64,
                    frequency: node.f32("frequency"),
                    octaves: node.int("octaves").max(1) as usize,
                    lacunarity: node.f32("lacunarity"),
                    gain: node.f32("gain"),
                    color1: node.color("color1"),
                    color2: node.color("color2"),
                    angle_deg: node.f32("angle"),
                    center: node.vec2("center"),
                    radius: node.f32("radius"),
                    softness: node.f32("softness"),
                    size: node.int("size").max(1) as usize,
                };
                flat(ops::generate(&p))
            }
            NodeKind::Write => {
                let src = img(0)?;
                self.writes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(WriteRecord {
                        node: node.id,
                        path: node.str("path"),
                        frame,
                        image: src.clone(),
                    });
                Ok(Output::Image(src))
            }
        }
    }
}

/// Region a node needs from one of its inputs to produce `roi` of its output.
fn input_roi(
    node: &CompNode,
    port: usize,
    roi: Option<RegionOfInterest>,
) -> Option<RegionOfInterest> {
    let roi = roi?;
    match node.kind {
        NodeKind::Crop => {
            let full = RegionOfInterest::new(
                0,
                0,
                node.int("width").max(0) as usize,
                node.int("height").max(0) as usize,
            );
            let r = RegionOfInterest::new(
                roi.x,
                roi.y,
                roi.width.min(full.width),
                roi.height.min(full.height),
            );
            Some(r.offset(node.int("x"), node.int("y")))
        }
        NodeKind::Blur => {
            let kind = node
                .param("kind")
                .and_then(BlurKind::from_param)
                .unwrap_or(BlurKind::Gaussian);
            Some(roi.expanded(ops::filter::blur_radius(node.f32("size"), kind)))
        }
        NodeKind::Sharpen => Some(roi.expanded(ops::filter::blur_radius(
            node.f32("size"),
            BlurKind::Gaussian,
        ))),
        NodeKind::Erode | NodeKind::Dilate => Some(roi.expanded(node.int("size").max(0) as usize)),
        NodeKind::EdgeBlur => Some(roi.expanded(
            ops::filter::blur_radius(node.f32("size"), BlurKind::Gaussian)
                + node.int("edge_width").max(0) as usize,
        )),
        NodeKind::ZDefocus => {
            if port == 0 {
                Some(roi.expanded(ops::filter::blur_radius(
                    node.f32("max_blur"),
                    BlurKind::Gaussian,
                )))
            } else {
                Some(roi)
            }
        }
        NodeKind::Merge
        | NodeKind::Keyer
        | NodeKind::ColorCorrect
        | NodeKind::Grade
        | NodeKind::Copy
        | NodeKind::Shuffle
        | NodeKind::Premult
        | NodeKind::Unpremult
        | NodeKind::Invert
        | NodeKind::Clamp
        | NodeKind::Expression
        | NodeKind::Write => Some(roi),
        NodeKind::Read
        | NodeKind::Procedural
        | NodeKind::Transform
        | NodeKind::Reformat
        | NodeKind::Roto
        | NodeKind::DeepMerge
        | NodeKind::DeepToImage
        | NodeKind::DeepFromImage
        | NodeKind::DeepHoldout => None,
    }
}

/// Convenience: the pass kinds a previs-driven template graph reads.
pub fn template_passes() -> &'static [PassKind] {
    &[PassKind::Beauty, PassKind::Depth]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_model::GeneratorKind;

    fn chain() -> (CompGraph, NodeId, NodeId, NodeId) {
        let mut g = CompGraph::new();
        let r = g
            .add(CompNode::read_generator(
                "r",
                GeneratorKind::Checkerboard,
                32,
                32,
            ))
            .unwrap();
        let b = g.add(CompNode::blur("b", 1.0, BlurKind::Gaussian)).unwrap();
        let c = g.add(CompNode::new("c", NodeKind::Invert)).unwrap();
        g.connect(r, 0, b, 0).unwrap();
        g.connect(b, 0, c, 0).unwrap();
        (g, r, b, c)
    }

    #[test]
    fn cache_hits_and_invalidation() {
        let (mut g, r, b, c) = chain();
        let ev = Evaluator::default();
        ev.evaluate(&g, c, 0).unwrap();
        assert_eq!(ev.stats().nodes_evaluated, 3);
        ev.evaluate(&g, c, 0).unwrap();
        let s = ev.stats();
        assert_eq!(s.nodes_evaluated, 3);
        assert_eq!(s.cache_hits, 1);
        g.set_param(b, "size", 2.0f64).unwrap();
        ev.reset_stats();
        ev.evaluate(&g, c, 0).unwrap();
        let s = ev.stats();
        assert_eq!(s.cache_hits, 1, "read served from cache");
        assert_eq!(s.evaluated, vec![b, c]);
        // invalidating an upstream node does not touch cached downstream results
        ev.invalidate(r);
        ev.reset_stats();
        ev.evaluate(&g, c, 0).unwrap();
        assert_eq!(ev.stats().cache_hits, 1);
        assert!(ev.stats().evaluated.is_empty());
        ev.evaluate(&g, r, 0).unwrap();
        assert_eq!(ev.stats().evaluated, vec![r]);
        // different frame is a different key
        ev.reset_stats();
        ev.evaluate(&g, c, 1).unwrap();
        assert_eq!(ev.stats().nodes_evaluated, 3);
        assert!(ev.cache_len() >= 6);
        ev.clear_cache();
        assert_eq!(ev.cache_len(), 0);
    }

    #[test]
    fn bypass_and_missing_input() {
        let (mut g, _r, b, c) = chain();
        g.node_mut(b).unwrap().bypass = true;
        let ev = Evaluator::default();
        let out = ev.evaluate(&g, c, 0).unwrap();
        let expect = ops::invert(&Image::checkerboard(
            32,
            32,
            16,
            [0.5, 0.5, 0.5, 1.0],
            [0.0, 0.0, 0.0, 1.0],
        ));
        assert!(out.approx_eq(&expect, 1e-6));
        let mut g2 = CompGraph::new();
        let m = g2.add(CompNode::merge("m", MergeOp::Over)).unwrap();
        assert!(matches!(
            ev.evaluate(&g2, m, 0),
            Err(Error::MissingInput { .. })
        ));
        assert!(matches!(
            ev.evaluate(&g2, NodeId::new(), 0),
            Err(Error::NodeNotFound(_))
        ));
    }

    #[test]
    fn files_and_writes() {
        let mut ev = Evaluator::default();
        ev.register_file("plate.exr", Image::solid(4, 4, [1.0, 0.5, 0.0, 1.0]));
        let mut g = CompGraph::new();
        let r = g
            .add(CompNode::read(
                "r",
                ReadSource::File {
                    path: "plate.exr".into(),
                },
            ))
            .unwrap();
        let w = g.add(CompNode::write("w", "out.exr")).unwrap();
        g.connect(r, 0, w, 0).unwrap();
        let out = ev.evaluate(&g, w, 3).unwrap();
        assert_eq!(out.get_rgba(0, 0), [1.0, 0.5, 0.0, 1.0]);
        let writes = ev.take_writes();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].path, "out.exr");
        assert_eq!(writes[0].frame, 3);
        assert!(ev.writes().is_empty());
        let missing = g
            .add(CompNode::read(
                "m",
                ReadSource::File {
                    path: "nope".into(),
                },
            ))
            .unwrap();
        assert!(matches!(
            ev.evaluate(&g, missing, 0),
            Err(Error::MissingSource(_))
        ));
    }

    #[test]
    fn roi_crop_matches_full() {
        let mut g = CompGraph::new();
        let scene = ucg_core::EntityId::new();
        let r = g
            .add(CompNode::read_previs(
                "r",
                scene,
                PassKind::Beauty,
                ucg_core::EntityId::nil(),
            ))
            .unwrap();
        let b = g.add(CompNode::blur("b", 1.5, BlurKind::Gaussian)).unwrap();
        let c = g
            .add(
                CompNode::new("c", NodeKind::Crop)
                    .with_param("x", 20i64)
                    .with_param("y", 10i64)
                    .with_param("width", 30i64)
                    .with_param("height", 20i64),
            )
            .unwrap();
        g.connect(r, 0, b, 0).unwrap();
        g.connect(b, 0, c, 0).unwrap();
        let ev = Evaluator::with_synthetic();
        let full = ev.evaluate(&g, c, 0).unwrap();
        let roi = ev
            .evaluate_roi(&g, c, 0, RegionOfInterest::new(0, 0, 30, 20))
            .unwrap();
        assert!(full.approx_eq(&roi, 1e-5));
        assert_eq!(
            ev.stats().nodes_evaluated,
            6,
            "roi request is a separate cache entry"
        );
    }

    #[test]
    fn deep_chain() {
        let mut g = CompGraph::new();
        let beauty = g
            .add(CompNode::read_generator(
                "beauty",
                GeneratorKind::Solid,
                4,
                4,
            ))
            .unwrap();
        let depth = g
            .add(
                CompNode::read_generator("depth", GeneratorKind::Solid, 4, 4)
                    .with_param("color", [5.0f32, 5.0, 5.0, 1.0]),
            )
            .unwrap();
        let dfi = g
            .add(CompNode::new("dfi", NodeKind::DeepFromImage))
            .unwrap();
        let dti = g.add(CompNode::new("dti", NodeKind::DeepToImage)).unwrap();
        g.connect(beauty, 0, dfi, 0).unwrap();
        g.connect(depth, 0, dfi, 1).unwrap();
        g.connect(dfi, 0, dti, 0).unwrap();
        let ev = Evaluator::default();
        let d = ev.evaluate_deep(&g, dfi, 0).unwrap();
        assert_eq!(d.sample_count(), 16);
        assert_eq!(d.pixel(0, 0)[0].z, 5.0);
        let flat = ev.evaluate(&g, dti, 0).unwrap();
        assert_eq!(flat.get_rgba(1, 1), [0.5, 0.5, 0.5, 1.0]);
        assert!(matches!(
            ev.evaluate(&g, dfi, 0),
            Err(Error::OutputMismatch { .. })
        ));
    }
}
