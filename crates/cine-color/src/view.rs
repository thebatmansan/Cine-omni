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

/// How a picture is put through a display and view.
///
/// Two kinds, because two kinds of picture exist and one method does not serve
/// both:
///
/// * **Baked** — display-referred input (video, log). Its code values live in a
///   known, bounded range, so the transform is a 33³ lattice: what the window's
///   shader interpolates, with its error measured.
/// * **Exact** — scene-linear input. Scene light is unbounded: a highlight at
///   6.25 is ordinary, and a lattice over code values would have to clamp it or
///   stretch its domain absurdly. So the transform is applied exactly, per
///   pixel, by the processor itself. Slower, and correct.
///
/// The choice is made from the input's encoding, not from what the caller
/// hopes, and it travels with the result so a surface can say which it used.
#[derive(Clone)]
pub enum ViewTransform {
    Baked(BakedView),
    Exact {
        processor: Arc<crate::processor::Processor>,
        describe: String,
    },
}

impl std::fmt::Debug for ViewTransform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({})",
            self.describe(),
            if self.is_exact() { "exact" } else { "baked" }
        )
    }
}

impl ViewTransform {
    /// One pixel through the view.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        match self {
            ViewTransform::Baked(b) => b.lut.apply(rgb),
            ViewTransform::Exact { processor, .. } => processor.apply_rgb(&rgb),
        }
    }

    /// A whole picture through the view, in place. `channels` is the stride:
    /// RGBA data keeps its alpha, which no view transform touches.
    pub fn apply_image(&self, data: &mut [f32], channels: usize) {
        match self {
            ViewTransform::Exact { processor, .. } => processor.apply_image(data, channels),
            ViewTransform::Baked(b) => {
                for px in data.chunks_exact_mut(channels) {
                    let [r, g, bl] = b.lut.apply([px[0], px[1], px[2]]);
                    px[0] = r;
                    px[1] = g;
                    px[2] = bl;
                }
            }
        }
    }

    /// What it is, in words: `input → grade → display / view`.
    pub fn describe(&self) -> &str {
        match self {
            ViewTransform::Baked(b) => &b.describe,
            ViewTransform::Exact { describe, .. } => describe,
        }
    }

    /// The worst difference from the exact transform. Zero for the exact path,
    /// because it *is* the transform.
    pub fn worst_error(&self) -> f32 {
        match self {
            ViewTransform::Baked(b) => b.worst_error,
            ViewTransform::Exact { .. } => 0.0,
        }
    }

    /// Whether the transform is applied exactly rather than through a lattice.
    pub fn is_exact(&self) -> bool {
        matches!(self, ViewTransform::Exact { .. })
    }
}

/// The view for one input colorspace, by whichever method that input needs.
///
/// Errors say what could not be done, rather than answering `None` and leaving
/// the caller to guess whether the transform was impossible or the input
/// unknown.
pub fn view_for(
    config: &Config,
    input: &str,
    working: &str,
    cdl: Option<&ucg_schema::Cdl>,
    display: &str,
    view: &str,
) -> Result<ViewTransform, String> {
    view_for_graded(config, input, working, cdl, None, display, view)
}

/// The same, with the colourist's layered grade in the chain.
pub fn view_for_graded(
    config: &Config,
    input: &str,
    working: &str,
    cdl: Option<&ucg_schema::Cdl>,
    grade: Option<&crate::grade::GradeStack>,
    display: &str,
    view: &str,
) -> Result<ViewTransform, String> {
    let grade = grade.filter(|g| !g.is_empty());
    let encoding = config
        .colorspace(input)
        .ok_or_else(|| format!("`{input}` is not a colorspace in this project's config"))?
        .encoding
        .to_lowercase();
    let display_referred = matches!(
        encoding.as_str(),
        "sdr-video" | "hdr-video" | "edr-video" | "log"
    );
    // A grade whose effect depends on the pixel cannot be a lattice, so a
    // picture that would normally be baked is put through the exact chain
    // instead. Slower, and the only answer that is actually the grade.
    if display_referred && !grade.is_some_and(|g| g.varies_by_pixel()) {
        return bake_view_graded(config, input, working, cdl, grade, display, view)
            .map(ViewTransform::Baked)
            .ok_or_else(|| {
                format!(
                    "`{input}` cannot be baked into {display} / {view}: the transform does not compile"
                )
            });
    }
    // Scene-linear (or a config that declares no encoding, which ACES configs
    // do for scene-referred spaces): exact, because the values are unbounded.
    let processor =
        crate::ucg::viewing_processor_with(config, input, working, cdl, grade, display, view)
            .map_err(|e| format!("`{input}` through {display} / {view}: {e}"))?;
    let describe = format!(
        "{input} →{} {display} / {view} (exact, scene-linear)",
        graded_step(cdl, grade)
    );
    Ok(ViewTransform::Exact {
        processor: Arc::new(processor),
        describe,
    })
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
    bake_view_graded(config, input, working, cdl, None, display, view)
}

/// The same, with the colourist's grade baked in with everything else.
///
/// Refuses a grade whose effect varies by pixel — see
/// [`crate::GradeStack::varies_by_pixel`] — and the caller takes the exact
/// path instead. A *qualifier* is a function of the input colour, which is what
/// a lattice is, so it could in principle be baked; it is not, because a
/// qualifier's edges are where its whole point lives and 33 samples a side
/// would smear them. A *mask* cannot be baked or carried in a chain at all,
/// and is refused by [`crate::ucg::viewing_processor_with`].
pub fn bake_view_graded(
    config: &Config,
    input: &str,
    working: &str,
    cdl: Option<&ucg_schema::Cdl>,
    grade: Option<&crate::grade::GradeStack>,
    display: &str,
    view: &str,
) -> Option<BakedView> {
    if grade.is_some_and(|g| g.varies_by_pixel()) {
        return None;
    }
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
    let processor =
        crate::ucg::viewing_processor_with(config, input, working, cdl, grade, display, view)
            .ok()?;
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
    let describe = format!("{input} →{} {display} / {view}", graded_step(cdl, grade));
    Some(BakedView {
        lut: Arc::new(lut),
        worst_error,
        describe,
    })
}

/// The middle of the chain's name: what is applied in the working space, if
/// anything. Said rather than implied — a description that left the grade out
/// would be a graded picture describing itself as ungraded.
fn graded_step(
    cdl: Option<&ucg_schema::Cdl>,
    grade: Option<&crate::grade::GradeStack>,
) -> &'static str {
    match (cdl.is_some(), grade.is_some()) {
        (false, false) => "",
        (true, false) => " CDL →",
        (false, true) => " grade →",
        (true, true) => " CDL → grade →",
    }
}

/// A stable name for a grade, so two clips graded differently never share a
/// cached transform.
///
/// Serialised rather than hashed field by field: a stack is a tree of layers,
/// masks and curves, and a fingerprint that walked only the fields somebody
/// remembered would collide the moment a new one was added — which is the
/// kind of cache bug that shows up as one shot wearing another's grade.
pub fn grade_fingerprint(grade: Option<&crate::grade::GradeStack>) -> String {
    let Some(g) = grade.filter(|g| !g.is_empty()) else {
        return "nograde".into();
    };
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(g).unwrap_or_default().hash(&mut h);
    format!("g{:x}", h.finish())
}

/// A short, stable name for a CDL, so a cache of bakes can tell two grades
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
