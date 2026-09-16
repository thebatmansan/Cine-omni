//! What a viewer loses by showing a lattice instead of the transform.
//!
//! The window cannot run the ACES 2.0 output transform per pixel per frame, so
//! it interpolates a baked lattice on the GPU. That is an approximation, and
//! the size of it is a measured number rather than a matter of taste: this
//! measures the error against the exact transform, and what baking costs, for
//! the sizes worth considering.

use cine_color::lut::{Interpolation, Lut3D};
use cine_color::ocio::Config;
use std::time::Instant;

const DOMAIN: (f32, f32) = (-0.125, 1.125);

#[test]
fn the_lattice_the_window_uses_is_measured_against_the_transform() {
    let config = Config::parse(cine_color::ucg::BUNDLED_CONFIG).unwrap();
    let processor = config
        .display_processor(
            "Rec.1886 Rec.709 - Display",
            "sRGB - Display",
            "ACES 2.0 - SDR 100 nits (Rec.709)",
        )
        .expect("video through the ACES SDR view");
    let mut shipped = None;
    for size in [17usize, 33, 49, 65] {
        let at = Instant::now();
        let mut lut = Lut3D::bake_domain(&processor, size, [DOMAIN.0; 3], [DOMAIN.1; 3]);
        let baked = at.elapsed();
        lut.interpolation = Interpolation::Trilinear;
        let trilinear = lut.worst_error(&processor, 4096);
        lut.interpolation = Interpolation::Tetrahedral;
        let tetrahedral = lut.worst_error(&processor, 4096);
        println!(
            "MEASURED {size}³: worst {:.4} trilinear, {:.4} tetrahedral (of full scale; \
             {:.1} and {:.1} of 255); baked in {:.0} ms, {:.1} MB on the GPU",
            trilinear,
            tetrahedral,
            trilinear * 255.0,
            tetrahedral * 255.0,
            baked.as_secs_f64() * 1000.0,
            (size.pow(3) * 16) as f64 / 1e6,
        );
        if size == 33 {
            shipped = Some(trilinear);
        }
    }
    // What the window ships with. A viewer may be approximate; it may not be
    // approximate by an amount nobody has measured.
    let worst = shipped.expect("33³ measured");
    assert!(
        worst < 0.02,
        "the 33³ lattice is {worst} from the transform — too far to call it the view"
    );
}

/// Where the error is, since the number alone does not say whether a colourist
/// would see it: the bulk of the picture, or the corners nobody grades in.
#[test]
fn the_lattice_is_closest_where_the_picture_is() {
    let config = Config::parse(cine_color::ucg::BUNDLED_CONFIG).unwrap();
    let processor = config
        .display_processor(
            "Rec.1886 Rec.709 - Display",
            "sRGB - Display",
            "ACES 2.0 - SDR 100 nits (Rec.709)",
        )
        .unwrap();
    let mut lut = Lut3D::bake_domain(&processor, 33, [DOMAIN.0; 3], [DOMAIN.1; 3]);
    lut.interpolation = Interpolation::Trilinear;
    // In-range code values only: what a legal picture actually contains.
    let mut in_range = Lut3D::bake_domain(&processor, 33, [DOMAIN.0; 3], [DOMAIN.1; 3]);
    in_range.interpolation = Interpolation::Trilinear;
    let mut worst_legal = 0.0f32;
    let phi = [0.819_172_5_f32, 0.671_043_6, 0.549_700_5];
    for i in 0..4096 {
        let rgb = [0, 1, 2].map(|c| ((i as f32 + 0.5) * phi[c]).fract());
        let exact = processor.apply_rgb(&rgb);
        let through = in_range.apply(rgb);
        for c in 0..3 {
            worst_legal = worst_legal.max((exact[c] - through[c]).abs());
        }
    }
    println!(
        "MEASURED 33³ within legal code values [0, 1]: worst {:.4} ({:.1} of 255)",
        worst_legal,
        worst_legal * 255.0
    );
    assert!(worst_legal < 0.01, "{worst_legal}");
}

/// A grade is part of the transform the viewer bakes, not something applied
/// afterwards: the lattice itself moves, and by the amount the CDL says.
#[test]
fn a_grade_changes_the_lattice_the_viewer_interpolates() {
    use cine_color::ucg::viewing_processor;
    use ucg_schema::Cdl;

    let config = Config::parse(cine_color::ucg::BUNDLED_CONFIG).unwrap();
    let (input, display, view) = (
        "Rec.1886 Rec.709 - Display",
        "sRGB - Display",
        "ACES 2.0 - SDR 100 nits (Rec.709)",
    );
    let plain = viewing_processor(&config, input, "ACEScg", None, display, view).unwrap();
    let warm = Cdl {
        slope: [1.2, 1.0, 0.8],
        offset: [0.0; 3],
        power: [1.0; 3],
        saturation: 1.0,
    };
    let graded = viewing_processor(&config, input, "ACEScg", Some(&warm), display, view).unwrap();

    // A mid grey through both: warmer, and by a visible amount.
    let grey = [0.5f32; 3];
    let (a, b) = (plain.apply_rgb(&grey), graded.apply_rgb(&grey));
    println!("MEASURED mid grey ungraded {a:?} → graded {b:?}");
    assert!(b[0] > a[0] + 0.02, "the red lift is visible: {a:?} → {b:?}");
    assert!(b[2] < a[2] - 0.02, "and the blue pull-down: {a:?} → {b:?}");

    // Baked, the lattice carries it and stays as close to its own transform as
    // the ungraded one does.
    let mut lut = Lut3D::bake_domain(&graded, 33, [DOMAIN.0; 3], [DOMAIN.1; 3]);
    lut.interpolation = Interpolation::Trilinear;
    let error = lut.worst_error(&graded, 4096);
    println!(
        "MEASURED graded lattice: worst {error:.4} of full scale ({:.1} of 255)",
        error * 255.0
    );
    assert!(error < 0.02, "{error}");

    // A neutral grade is not a grade: the chain is the ungraded one.
    let neutral = Cdl::default();
    let same = viewing_processor(&config, input, "ACEScg", Some(&neutral), display, view).unwrap();
    for x in [[0.1f32; 3], [0.5; 3], [0.9, 0.2, 0.4]] {
        let (p, s) = (plain.apply_rgb(&x), same.apply_rgb(&x));
        for c in 0..3 {
            assert!((p[c] - s[c]).abs() < 1e-5, "{p:?} vs {s:?}");
        }
    }
}
