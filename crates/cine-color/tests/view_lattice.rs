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
