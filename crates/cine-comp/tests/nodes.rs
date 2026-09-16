//! Node-level integration tests: every operation exercised through a
//! `CompGraph` and the `Evaluator`.

use cine_comp::ops;
use cine_comp::prelude::*;

fn solid(name: &str, w: usize, h: usize, color: [f32; 4]) -> CompNode {
    CompNode::read_generator(name, GeneratorKind::Solid, w, h).with_param("color", color)
}

#[test]
fn merge_over_half_red_over_blue_exact() {
    let mut g = CompGraph::new();
    let a = g.add(solid("a", 4, 4, [0.5, 0.0, 0.0, 0.5])).unwrap();
    let b = g.add(solid("b", 4, 4, [0.0, 0.0, 1.0, 1.0])).unwrap();
    let m = g.add(CompNode::merge("m", MergeOp::Over)).unwrap();
    g.connect(a, 0, m, 0).unwrap();
    g.connect(b, 0, m, 1).unwrap();
    let ev = Evaluator::default();
    let out = ev.evaluate(&g, m, 0).unwrap();
    assert_eq!(out.get_rgba(2, 2), [0.5, 0.0, 0.5, 1.0]);

    // Plus / Multiply / Mask numerically.
    for (op, expect) in [
        (MergeOp::Plus, [0.5, 0.0, 1.0, 1.5]),
        (MergeOp::Multiply, [0.0, 0.0, 0.0, 1.0]),
        (MergeOp::Mask, [0.0, 0.0, 0.5, 0.5]),
        (MergeOp::Stencil, [0.0, 0.0, 0.5, 0.5]),
    ] {
        g.set_param(m, "operation", op.param()).unwrap();
        let out = ev.evaluate(&g, m, 0).unwrap();
        assert_eq!(out.get_rgba(1, 1), expect, "{op:?}");
    }
    // mix 0 returns B unchanged
    g.set_param(m, "operation", MergeOp::Over.param()).unwrap();
    g.set_param(m, "mix", 0.0f64).unwrap();
    assert_eq!(
        ev.evaluate(&g, m, 0).unwrap().get_rgba(0, 0),
        [0.0, 0.0, 1.0, 1.0]
    );
}

#[test]
fn chroma_keyer_on_green_screen() {
    // green background with a red square in the middle, plus some spill on the square edge
    let mut plate = Image::solid(32, 32, [0.1, 0.9, 0.15, 1.0]);
    for y in 8..24 {
        for x in 8..24 {
            plate.set_rgba(x, y, [0.8, 0.2, 0.1, 1.0]);
        }
    }
    plate.set_rgba(8, 16, [0.6, 0.7, 0.2, 1.0]); // spill-contaminated edge pixel
    let mut ev = Evaluator::default();
    ev.register_file("plate", plate);
    let mut g = CompGraph::new();
    let r = g
        .add(CompNode::read(
            "plate",
            ReadSource::File {
                path: "plate".into(),
            },
        ))
        .unwrap();
    let k = g
        .add(
            CompNode::keyer("key", KeyerKind::Chroma)
                .with_param("key_color", [0.1f32, 0.9, 0.15, 1.0]),
        )
        .unwrap();
    g.connect(r, 0, k, 0).unwrap();
    let out = ev.evaluate(&g, k, 0).unwrap();
    for y in 0..32 {
        for x in 0..32 {
            let inside = (8..24).contains(&x) && (8..24).contains(&y);
            let a = out.get_alpha(x, y);
            if inside && !(x == 8 && y == 16) {
                assert!(a > 0.99, "alpha {a} inside at {x},{y}");
                let p = out.get_rgba(x, y);
                assert!((p[0] - 0.8).abs() < 1e-5 && (p[1] - 0.2).abs() < 1e-5);
            } else if !inside {
                assert!(a < 0.01, "alpha {a} outside at {x},{y}");
                assert_eq!(
                    out.get_rgba(x, y)[1],
                    0.0,
                    "keyed pixels are premultiplied to black"
                );
            }
        }
    }
    // spill removed: green no longer exceeds the mean of red and blue
    let p = out.get_rgba(8, 16);
    let a = p[3];
    assert!(a > 0.0);
    assert!(
        p[1] / a <= (p[0] + p[2]) / (2.0 * a) + 1e-5,
        "spill {:?}",
        p
    );

    // screen (IBK style) keyer behaves the same on this plate
    g.set_param(k, "kind", KeyerKind::Screen.param()).unwrap();
    let out = ev.evaluate(&g, k, 0).unwrap();
    assert!(out.get_alpha(16, 16) > 0.99);
    assert!(out.get_alpha(2, 2) < 0.01);

    // difference keyer against a clean plate
    let clean = g
        .add(solid("clean", 32, 32, [0.1, 0.9, 0.15, 1.0]))
        .unwrap();
    g.set_param(k, "kind", KeyerKind::Difference.param())
        .unwrap();
    g.connect(clean, 0, k, 1).unwrap();
    let out = ev.evaluate(&g, k, 0).unwrap();
    assert!(out.get_alpha(16, 16) > 0.99);
    assert!(out.get_alpha(2, 2) < 0.01);
}

#[test]
fn blur_preserves_mean_and_transform_translates_exactly() {
    let mut ev = Evaluator::default();
    let mut src = Image::rgba(48, 48);
    for y in 20..28 {
        for x in 20..28 {
            src.set_rgba(x, y, [1.0, 0.5, 0.25, 1.0]);
        }
    }
    let mean_before = src.channel_mean(0);
    ev.register_file("src", src.clone());
    let mut g = CompGraph::new();
    let r = g
        .add(CompNode::read("r", ReadSource::File { path: "src".into() }))
        .unwrap();
    let b = g.add(CompNode::blur("b", 2.0, BlurKind::Gaussian)).unwrap();
    g.connect(r, 0, b, 0).unwrap();
    let out = ev.evaluate(&g, b, 0).unwrap();
    assert!((out.channel_mean(0) - mean_before).abs() < 1e-6);
    assert!((out.channel_mean(3) - src.channel_mean(3)).abs() < 1e-6);
    assert!(out.get_rgba(24, 24)[0] < 1.0 && out.get_rgba(18, 24)[0] > 0.0);
    g.set_param(b, "kind", BlurKind::Box.param()).unwrap();
    let out = ev.evaluate(&g, b, 0).unwrap();
    assert!((out.channel_mean(0) - mean_before).abs() < 1e-6);

    let t = g
        .add(CompNode::transform("t", [5.0, 3.0], 0.0, [1.0, 1.0]))
        .unwrap();
    g.connect(r, 0, t, 0).unwrap();
    let out = ev.evaluate(&g, t, 0).unwrap();
    // The window moves with the picture: every source pixel is still there,
    // five right and three down, and nothing was cropped off the far edge.
    assert_eq!(out.window(), (5, 3, src.width, src.height));
    for y in 0..src.height {
        for x in 0..src.width {
            assert_eq!(
                out.canvas_rgba(x as i64 + 5, y as i64 + 3),
                src.get_rgba(x, y),
                "at {x},{y}"
            );
        }
    }
    assert_eq!(
        out.canvas_rgba(0, 0),
        [0.0; 4],
        "nothing outside the window"
    );
    let bic = g
        .add(
            CompNode::transform("bic", [5.0, 3.0], 0.0, [1.0, 1.0]).with_param("filter", "bicubic"),
        )
        .unwrap();
    g.connect(r, 0, bic, 0).unwrap();
    let out2 = ev.evaluate(&g, bic, 0).unwrap();
    assert!(out2.approx_eq(&out, 1e-5));
}

#[test]
fn roto_polygon_area_within_two_percent() {
    // pentagon-ish polygon, analytic area via shoelace
    let pts = vec![
        [10.0, 8.0],
        [50.0, 12.0],
        [58.0, 40.0],
        [30.0, 56.0],
        [6.0, 36.0],
    ];
    let expected = ops::roto::polygon_area(&pts).abs() as f64;
    let shape = Shape::polygon(pts).with_feather(3.0);
    let mut g = CompGraph::new();
    let r = g.add(CompNode::roto("roto", vec![shape], 64, 64)).unwrap();
    let ev = Evaluator::default();
    let mask = ev.evaluate(&g, r, 0).unwrap();
    assert_eq!(mask.channels, ChannelSet::Alpha);
    let area = mask.channel_sum(0);
    assert!(
        (area - expected).abs() / expected < 0.02,
        "area {area} expected {expected}"
    );
    // mask feeds a merge mask port
    let a = g.add(solid("a", 64, 64, [1.0, 0.0, 0.0, 1.0])).unwrap();
    let b = g.add(solid("b", 64, 64, [0.0, 0.0, 1.0, 1.0])).unwrap();
    let m = g.add(CompNode::merge("m", MergeOp::Over)).unwrap();
    g.connect(a, 0, m, 0).unwrap();
    g.connect(b, 0, m, 1).unwrap();
    g.connect(r, 0, m, 2).unwrap();
    let out = ev.evaluate(&g, m, 0).unwrap();
    assert_eq!(out.get_rgba(30, 30), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(out.get_rgba(1, 60), [0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn deep_flatten_and_holdout_through_graph() {
    let mut ev = Evaluator::default();
    let front = Image::solid(2, 2, [0.4, 0.0, 0.0, 0.4]);
    let back = Image::solid(2, 2, [0.0, 0.0, 0.8, 0.8]);
    ev.register_file("front", front);
    ev.register_file("back", back);
    let mut g = CompGraph::new();
    let f = g
        .add(CompNode::read(
            "front",
            ReadSource::File {
                path: "front".into(),
            },
        ))
        .unwrap();
    let b = g
        .add(CompNode::read(
            "back",
            ReadSource::File {
                path: "back".into(),
            },
        ))
        .unwrap();
    let zf = g.add(solid("zf", 2, 2, [1.0, 1.0, 1.0, 1.0])).unwrap();
    let zb = g.add(solid("zb", 2, 2, [5.0, 5.0, 5.0, 1.0])).unwrap();
    let df = g.add(CompNode::new("df", NodeKind::DeepFromImage)).unwrap();
    let db = g.add(CompNode::new("db", NodeKind::DeepFromImage)).unwrap();
    let dm = g.add(CompNode::new("dm", NodeKind::DeepMerge)).unwrap();
    let flat = g.add(CompNode::new("flat", NodeKind::DeepToImage)).unwrap();
    g.connect(f, 0, df, 0).unwrap();
    g.connect(zf, 0, df, 1).unwrap();
    g.connect(b, 0, db, 0).unwrap();
    g.connect(zb, 0, db, 1).unwrap();
    // merge order must not matter: put the back first
    g.connect(db, 0, dm, 0).unwrap();
    g.connect(df, 0, dm, 1).unwrap();
    g.connect(dm, 0, flat, 0).unwrap();
    let out = ev.evaluate(&g, flat, 0).unwrap();
    let manual = ops::merge_pixel(MergeOp::Over, [0.4, 0.0, 0.0, 0.4], [0.0, 0.0, 0.8, 0.8]);
    let got = out.get_rgba(1, 1);
    for i in 0..4 {
        assert!((got[i] - manual[i]).abs() < 1e-6, "{got:?} vs {manual:?}");
    }
    // holdout: an opaque plane at z=3 hides the back samples entirely
    let plane = g.add(solid("plane", 2, 2, [0.0, 0.0, 0.0, 1.0])).unwrap();
    let zp = g.add(solid("zp", 2, 2, [3.0, 3.0, 3.0, 1.0])).unwrap();
    let dp = g.add(CompNode::new("dp", NodeKind::DeepFromImage)).unwrap();
    let ho = g.add(CompNode::new("ho", NodeKind::DeepHoldout)).unwrap();
    g.connect(plane, 0, dp, 0).unwrap();
    g.connect(zp, 0, dp, 1).unwrap();
    g.connect(dm, 0, ho, 0).unwrap();
    g.connect(dp, 0, ho, 1).unwrap();
    let held = ev.evaluate_deep(&g, ho, 0).unwrap();
    assert_eq!(held.sample_count(), 4, "only the front samples survive");
    assert!(held.samples.iter().all(|s| s.len() == 1 && s[0].z == 1.0));
    // deep ports reject flat images
    assert!(g.connect(f, 0, dm, 0).is_err());
}

#[test]
fn expression_and_channel_nodes() {
    let mut g = CompGraph::new();
    let s = g.add(solid("s", 4, 2, [0.5, 0.25, 0.0, 1.0])).unwrap();
    let e = g
        .add(CompNode::expression(
            "e",
            "r*0.5+g",
            "",
            "x/(w-1)",
            "min(a, 0.5)",
        ))
        .unwrap();
    g.connect(s, 0, e, 0).unwrap();
    let ev = Evaluator::default();
    let out = ev.evaluate(&g, e, 0).unwrap();
    let p = out.get_rgba(3, 1);
    assert!((p[0] - 0.5).abs() < 1e-6);
    assert_eq!(p[1], 0.25);
    assert!((p[2] - 1.0).abs() < 1e-6);
    assert_eq!(p[3], 0.5);
    g.set_param(e, "r", "pow(2,").unwrap();
    assert!(matches!(ev.evaluate(&g, e, 0), Err(Error::Expr(_))));

    let sh = g
        .add(
            CompNode::new("sh", NodeKind::Shuffle)
                .with_param("r", "a")
                .with_param("a", "one"),
        )
        .unwrap();
    g.connect(s, 0, sh, 0).unwrap();
    assert_eq!(
        ev.evaluate(&g, sh, 0).unwrap().get_rgba(0, 0),
        [1.0, 0.25, 0.0, 1.0]
    );
    let un = g.add(CompNode::new("un", NodeKind::Unpremult)).unwrap();
    let half = g.add(solid("half", 4, 2, [0.5, 0.5, 0.5, 0.5])).unwrap();
    g.connect(half, 0, un, 0).unwrap();
    assert_eq!(
        ev.evaluate(&g, un, 0).unwrap().get_rgba(0, 0),
        [1.0, 1.0, 1.0, 0.5]
    );
    let pm = g.add(CompNode::new("pm", NodeKind::Premult)).unwrap();
    g.connect(un, 0, pm, 0).unwrap();
    assert_eq!(
        ev.evaluate(&g, pm, 0).unwrap().get_rgba(0, 0),
        [0.5, 0.5, 0.5, 0.5]
    );
    let inv = g.add(CompNode::new("inv", NodeKind::Invert)).unwrap();
    g.connect(s, 0, inv, 0).unwrap();
    assert_eq!(
        ev.evaluate(&g, inv, 0).unwrap().get_rgba(0, 0),
        [0.5, 0.75, 1.0, 1.0]
    );
    let cl = g
        .add(CompNode::new("cl", NodeKind::Clamp).with_param("max", 0.3f64))
        .unwrap();
    g.connect(s, 0, cl, 0).unwrap();
    assert_eq!(
        ev.evaluate(&g, cl, 0).unwrap().get_rgba(0, 0),
        [0.3, 0.25, 0.0, 0.3]
    );
    let cp = g
        .add(CompNode::new("cp", NodeKind::Copy).with_param("channels", "alpha"))
        .unwrap();
    g.connect(s, 0, cp, 0).unwrap();
    g.connect(half, 0, cp, 1).unwrap();
    assert_eq!(
        ev.evaluate(&g, cp, 0).unwrap().get_rgba(0, 0),
        [0.5, 0.25, 0.0, 0.5]
    );
    let grade = g
        .add(CompNode::new("grade", NodeKind::Grade).with_param("gain", [2.0f32, 1.0, 1.0, 1.0]))
        .unwrap();
    g.connect(s, 0, grade, 0).unwrap();
    let gp = ev.evaluate(&g, grade, 0).unwrap().get_rgba(0, 0);
    assert!((gp[0] - 1.0).abs() < 1e-6 && (gp[1] - 0.25).abs() < 1e-6);
}

#[test]
fn procedural_crop_reformat_and_morphology() {
    let mut g = CompGraph::new();
    let n = g
        .add(CompNode::procedural("noise", ProceduralKind::Noise, 64, 32).with_param("seed", 3i64))
        .unwrap();
    let ev = Evaluator::default();
    let img = ev.evaluate(&g, n, 0).unwrap();
    assert_eq!((img.width, img.height), (64, 32));
    assert!(img.channel_mean(0) > 0.2 && img.channel_mean(0) < 0.8);
    let c = g
        .add(
            CompNode::new("crop", NodeKind::Crop)
                .with_param("x", 10i64)
                .with_param("y", 4i64)
                .with_param("width", 8i64)
                .with_param("height", 8i64),
        )
        .unwrap();
    g.connect(n, 0, c, 0).unwrap();
    let cropped = ev.evaluate(&g, c, 0).unwrap();
    assert_eq!((cropped.width, cropped.height), (8, 8));
    assert_eq!(cropped.get_rgba(0, 0), img.get_rgba(10, 4));
    let rf = g
        .add(
            CompNode::new("rf", NodeKind::Reformat)
                .with_param("width", 16i64)
                .with_param("height", 8i64),
        )
        .unwrap();
    g.connect(n, 0, rf, 0).unwrap();
    let small = ev.evaluate(&g, rf, 0).unwrap();
    assert_eq!((small.width, small.height), (16, 8));
    for kind in [
        ProceduralKind::Gradient,
        ProceduralKind::Checker,
        ProceduralKind::Radial,
        ProceduralKind::Cellular,
    ] {
        g.set_param(n, "kind", kind.param()).unwrap();
        let out = ev.evaluate(&g, n, 0).unwrap();
        assert!(out.data.iter().all(|v| (0.0..=1.0).contains(v)), "{kind:?}");
    }
    // erode/dilate/edge blur on a roto matte
    let roto = g
        .add(CompNode::roto(
            "roto",
            vec![Shape::rect(8.0, 8.0, 8.0, 8.0)],
            32,
            32,
        ))
        .unwrap();
    let er = g
        .add(CompNode::new("er", NodeKind::Erode).with_param("size", 2i64))
        .unwrap();
    let di = g
        .add(CompNode::new("di", NodeKind::Dilate).with_param("size", 2i64))
        .unwrap();
    let eb = g.add(CompNode::new("eb", NodeKind::EdgeBlur)).unwrap();
    let sh = g.add(CompNode::new("sh", NodeKind::Sharpen)).unwrap();
    for n in [er, di, eb, sh] {
        g.connect(roto, 0, n, 0).unwrap();
    }
    assert!((ev.evaluate(&g, er, 0).unwrap().channel_sum(3) - 16.0).abs() < 1e-4);
    assert!((ev.evaluate(&g, di, 0).unwrap().channel_sum(3) - 144.0).abs() < 1e-4);
    let e = ev.evaluate(&g, eb, 0).unwrap();
    assert_eq!(e.get_alpha(12, 12), 1.0);
    assert!(e.get_alpha(8, 12) < 1.0 && e.get_alpha(8, 12) > 0.0);
    assert!(ev.evaluate(&g, sh, 0).unwrap().get_alpha(12, 12) >= 1.0);
}

#[test]
fn display_and_multilayer_output() {
    let provider = SyntheticPassProvider::new(32, 24);
    let ml = provider.render_all(0);
    assert_eq!(
        ml.layer_names(),
        vec!["beauty", "depth", "normal", "id", "motion"]
    );
    let bytes = ml.beauty().unwrap().to_display_u8();
    assert_eq!(bytes.len(), 32 * 24 * 4);
    assert!(bytes.chunks(4).all(|p| p[3] == 255));
}

/// A foreground that hangs over the background's edge is part of the picture.
///
/// The merge used to crop the foreground to the background's window, so the
/// overhang was discarded before anything downstream could see it — the audit's
/// "no union bounding box". The result is now the union of the two windows,
/// as OpenEXR and every compositor mean it.
#[test]
fn a_foreground_over_the_edge_survives_the_merge() {
    use cine_comp::image::Image;
    use cine_comp::ops;

    let mut background = Image::rgba(64, 64);
    for y in 0..64 {
        for x in 0..64 {
            background.set_rgba(x, y, [0.0, 0.0, 0.5, 1.0]);
        }
    }
    // An opaque patch, half of it beyond the background's right edge.
    let mut foreground = Image::rgba(32, 32);
    for y in 0..32 {
        for x in 0..32 {
            foreground.set_rgba(x, y, [1.0, 0.0, 0.0, 1.0]);
        }
    }
    let foreground = foreground.placed(48, 16);

    let out = ops::merge(MergeOp::Over, &foreground, &background, None, 1.0);
    assert_eq!(
        out.window(),
        (0, 0, 80, 64),
        "the result covers both: the background, and the foreground's overhang"
    );
    // Over the background.
    assert_eq!(out.canvas_rgba(50, 20), [1.0, 0.0, 0.0, 1.0]);
    // Beyond the background's edge, where the old behaviour lost it.
    assert_eq!(
        out.canvas_rgba(70, 20),
        [1.0, 0.0, 0.0, 1.0],
        "the overhang is still there"
    );
    // Background where the foreground is not.
    assert_eq!(out.canvas_rgba(10, 10), [0.0, 0.0, 0.5, 1.0]);
    // And nothing was invented where neither image is.
    assert_eq!(out.canvas_rgba(70, 60), [0.0; 4]);
}

/// The region pulled back through a transform is the region it reads: a small
/// window of a scaled-up plate costs a small window of the source.
#[test]
fn a_transform_pulls_only_the_source_region_it_reads() {
    use cine_comp::image::RegionOfInterest;
    use cine_comp::ops;

    let t = ops::Transform2D {
        translate: [100.0, 50.0],
        scale: [4.0, 4.0],
        center: Some([0.0, 0.0]),
        ..Default::default()
    };
    let roi = RegionOfInterest::new(200, 150, 40, 40);
    let source = ops::source_region_for(&t, roi);
    // (200,150) maps back to ((200-100)/4, (150-50)/4) = (25, 25), and 40
    // destination pixels are 10 source pixels, plus the filter's reach.
    assert!(source.x <= 25 && source.y <= 25, "{source:?}");
    assert!(source.width <= 14 && source.height <= 14, "{source:?}");
    assert!(source.width >= 10 && source.height >= 10, "{source:?}");
}
