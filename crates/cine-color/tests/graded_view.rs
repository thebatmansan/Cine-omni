//! A clip's grade in the view chain, and the one shape of grade that cannot be.
//!
//! `view_for_graded` is the single function the native window, the browser's
//! frames and the render all use, so what it can and cannot carry is what all
//! three can and cannot show. The interesting boundary is not the maths — that
//! is tested in `grade.rs` — but *where* a grade is a function of.
//!
//! * A **qualifier** picks by the colour a pixel already is. A processor is
//!   handed exactly that, so a qualified grade is exact through the chain.
//! * A **mask** picks by where the pixel is. A processor is never told, and
//!   neither is `Processor::apply_image`, which walks pixels with no
//!   coordinates. So a masked grade cannot be carried at all, and the honest
//!   answer is a refusal that names it — not an evenly graded picture presented
//!   as a masked one.

use cine_color::grade::{GradeLayer, GradeOp, GradeStack};
use cine_color::mask::{HslQualifier, Mask};
use cine_color::ocio::Config;
use cine_color::view::{view_for_graded, ViewTransform};

const DISPLAY: &str = "sRGB - Display";
const VIEW: &str = "ACES 2.0 - SDR 100 nits (Rec.709)";
const VIDEO: &str = "Rec.1886 Rec.709 - Display";
const SCENE: &str = "ACEScct";

fn config() -> Config {
    Config::parse(cine_color::ucg::BUNDLED_CONFIG).expect("the bundled ACES config")
}

fn one_layer(layer: GradeLayer) -> GradeStack {
    GradeStack {
        layers: vec![layer],
        ..Default::default()
    }
}

fn exposure(stops: f64) -> Vec<GradeOp> {
    vec![GradeOp::Exposure { stops }]
}

#[test]
fn a_plain_primary_goes_through_and_changes_the_picture() {
    let c = config();
    let grade = one_layer(GradeLayer {
        name: "primary".into(),
        ops: exposure(1.0),
        ..Default::default()
    });
    let plain =
        view_for_graded(&c, VIDEO, "ACEScg", None, None, DISPLAY, VIEW).expect("the ungraded view");
    let graded = view_for_graded(&c, VIDEO, "ACEScg", None, Some(&grade), DISPLAY, VIEW)
        .expect("a primary grade is carried");
    // Display-referred input, no per-pixel variation: both are lattices, and
    // the grade is baked in with everything else.
    let (ViewTransform::Baked(a), ViewTransform::Baked(b)) = (&plain, &graded) else {
        panic!("video through a view is baked: {plain:?} / {graded:?}");
    };
    assert!(
        b.describe.contains("grade"),
        "the description says the grade is in the chain: {}",
        b.describe
    );
    let mid = [0.5f32, 0.5, 0.5];
    assert_ne!(
        a.lut.apply(mid),
        b.lut.apply(mid),
        "a stop of exposure must change mid grey"
    );
}

#[test]
fn a_qualified_grade_is_exact_rather_than_baked() {
    // A qualifier's edges are where its whole point lives, and 33 samples a
    // side would smear them, so the exact chain is taken instead of a lattice.
    let c = config();
    let grade = one_layer(GradeLayer {
        name: "the skin".into(),
        ops: exposure(0.5),
        qualifier: Some(HslQualifier::default()),
        ..Default::default()
    });
    let t = view_for_graded(&c, VIDEO, "ACEScg", None, Some(&grade), DISPLAY, VIEW)
        .expect("a qualified grade is a function of the colour, so it is carried");
    assert!(
        matches!(t, ViewTransform::Exact { .. }),
        "a qualified grade is not a lattice: {t:?}"
    );
}

#[test]
fn a_masked_grade_is_refused_by_name_rather_than_centred() {
    // The defect this exists to prevent: a processor maps a colour to a colour
    // and is never told where the pixel was, so evaluating a mask here would
    // apply it at one fixed point for the whole frame — an evenly graded
    // picture that claims to be a masked one.
    let c = config();
    for input in [VIDEO, SCENE] {
        let grade = one_layer(GradeLayer {
            name: "the window".into(),
            ops: exposure(1.0),
            mask: Some(Mask::circle([0.5, 0.5], 0.25, 0.1)),
            ..Default::default()
        });
        let refused = view_for_graded(&c, input, "ACEScg", None, Some(&grade), DISPLAY, VIEW)
            .expect_err("a masked grade cannot be carried in a view transform");
        assert!(
            refused.contains("mask") && refused.contains("the window"),
            "the refusal names the layer and what is wrong with it: {refused}"
        );
        assert!(
            refused.contains("apply_image"),
            "and names the path that would do it: {refused}"
        );
    }
}

#[test]
fn a_mask_on_a_layer_with_nothing_on_it_is_not_in_the_way() {
    // A layer that grades nothing cannot grade part of a frame either, so an
    // empty masked layer must not refuse a chain that is otherwise fine. This
    // is the shape a reactor or an import leaves behind.
    let c = config();
    let grade = GradeStack {
        layers: vec![
            GradeLayer {
                name: "an empty window".into(),
                ops: vec![],
                mask: Some(Mask::circle([0.5, 0.5], 0.25, 0.1)),
                ..Default::default()
            },
            GradeLayer {
                name: "primary".into(),
                ops: exposure(1.0),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let t = view_for_graded(&c, VIDEO, "ACEScg", None, Some(&grade), DISPLAY, VIEW)
        .expect("an empty masked layer is not a masked grade");
    assert!(matches!(t, ViewTransform::Baked(_)), "{t:?}");
}
