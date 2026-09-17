//! The project's view of a picture, baked: one definition for every surface.
//!
//! The native window shows the cut through the project's display and view, and
//! a render writes the cut through the same. If each built its own transform,
//! a delivered frame could differ from the frame a colourist signed off in the
//! window. So the transform is compiled and baked here once, and both use it.
//!
//! A bake is a 33³ lattice over code values `[−0.125, 1.125]` — limited-range
//! video reaches below 0 and above 1, and clipping them at the lattice edge
//! would hide exactly what a colourist looks for — interpolated trilinearly,
//! because that is what the window's shader does. Each bake carries the worst
//! difference between the lattice and the exact transform, measured, not
//! assumed.

use crate::lut::{Interpolation, Lut3D};
use crate::ocio::Config;
use std::collections::HashMap;
use std::sync::Arc;
use ucg_core::{EntityId, Graph};
use ucg_schema::{attrs, kinds, ColorMeta, Component};

/// Edge length of a baked view lattice.
pub const LATTICE_SIZE: usize = 33;

/// The code-value domain a lattice covers.
pub const LATTICE_DOMAIN: (f32, f32) = (-0.125, 1.125);

/// One clip's colorspace and grade through one display and view, baked.
#[derive(Clone, Debug)]
pub struct BakedView {
    pub lut: Arc<Lut3D>,
    /// The worst difference between the lattice and the exact transform.
    pub worst_error: f32,
    /// What it is, in words: `input → grade → display / view`.
    pub describe: String,
}

/// The project's colour config, parsed, with a key that changes when it does.
pub fn project_config(graph: &Graph, root: EntityId) -> Option<(Config, u64)> {
    let entity = crate::ucg::project_config(graph, root)?;
    let yaml = graph.node(entity)?.get_str("yaml")?.to_string();
    let config = Config::parse(&yaml).ok()?;
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        yaml.hash(&mut h);
        h.finish()
    };
    Some((config, key))
}

/// The config's default display and view: its active ones, or the first.
pub fn default_view(config: &Config) -> Option<(String, String)> {
    let display = config
        .active_displays
        .iter()
        .find(|d| config.displays.contains_key(*d))
        .cloned()
        .or_else(|| config.displays.keys().next().cloned())?;
    let views = config.views(&display).ok()?;
    let view = config
        .active_views
        .iter()
        .find(|v| views.iter().any(|x| x.name.eq_ignore_ascii_case(v)))
        .cloned()
        .or_else(|| views.first().map(|v| v.name.clone()))?;
    Some((display, view))
}

/// Media path → the input colorspace established for it.
pub fn input_colorspaces(graph: &Graph) -> HashMap<String, String> {
    graph
        .of_kind(kinds::ASSET_MEDIA)
        .iter()
        .filter_map(|n| {
            let path = n.get_str(attrs::MEDIA_PATH)?.to_string();
            let meta = ColorMeta::read_from(graph, n.id)?;
            Some((path, meta.input_colorspace))
        })
        .collect()
}

/// Bake `input → [grade →] display / view` and measure what baking cost.
///
/// `None` when the config cannot compile the transform, and — deliberately —
/// when the input colorspace is scene-linear: a lattice over code values would
/// be wrong for unbounded scene light, and showing the picture untransformed
/// while claiming a view is worse than showing it plainly.
pub fn bake_view(
    config: &Config,
    input: &str,
    working: &str,
    cdl: Option<&ucg_schema::Cdl>,
    display: &str,
    view: &str,
) -> Option<BakedView> {
    let encoding = config
        .colorspace(input)
        .map(|c| c.encoding.to_lowercase())
        .unwrap_or_default();
    if !matches!(
        encoding.as_str(),
        "sdr-video" | "hdr-video" | "edr-video" | "log"
    ) {
        return None;
    }
    let processor = crate::ucg::viewing_processor(config, input, working, cdl, display, view).ok()?;
    let mut lut = Lut3D::bake_domain(
        &processor,
        LATTICE_SIZE,
        [LATTICE_DOMAIN.0; 3],
        [LATTICE_DOMAIN.1; 3],
    );
    // Trilinear, because that is what the shader does; measuring tetrahedral
    // error would be measuring an interpolation nobody performs.
    lut.interpolation = Interpolation::Trilinear;
    let worst_error = lut.worst_error(&processor, 4096);
    let describe = match cdl {
        Some(_) => format!("{input} → grade → {display} / {view}"),
        None => format!("{input} → {display} / {view}"),
    };
    Some(BakedView {
        lut: Arc::new(lut),
        worst_error,
        describe,
    })
}

/// A short, stable name for a grade, so a cache of bakes can tell two grades
/// apart without holding the grade itself.
pub fn cdl_fingerprint(cdl: Option<&ucg_schema::Cdl>) -> String {
    let Some(cdl) = cdl else {
        return "ungraded".into();
    };
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for v in cdl
        .slope
        .iter()
        .chain(&cdl.offset)
        .chain(&cdl.power)
        .chain(std::iter::once(&cdl.saturation))
    {
        v.to_bits().hash(&mut h);
    }
    format!("{:016x}", h.finish())
}
