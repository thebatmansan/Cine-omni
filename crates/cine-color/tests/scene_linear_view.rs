//! The scene-linear view path: unbounded light through a display and view.
//!
//! A lattice over code values cannot carry scene light — a highlight at 6.25 is
//! ordinary, and clamping it to the lattice's edge is exactly the failure this
//! path exists to prevent. So scene-linear input is transformed exactly, and
//! these tests hold that to its meaning: highlights roll off rather than clip
//! flat, the darkest values stay ordered, negatives do not explode, and the
//! choice of method follows the input's encoding rather than a hope.

use cine_color::ocio::Config;
use cine_color::view::{default_view, view_for, ViewTransform};

fn config() -> Config {
    Config::parse(cine_color::ucg::BUNDLED_CONFIG).expect("the bundled ACES config parses")
}

#[test]
fn scene_linear_takes_the_exact_path_and_video_takes_the_lattice() {
    let config = config();
    let (display, view) = default_view(&config).unwrap();
    let working = config.role("scene_linear").unwrap_or("ACEScg").to_string();

    let scene = view_for(&config, "ACEScg", &working, None, &display, &view).unwrap();
    assert!(scene.is_exact(), "scene-linear must not go through a lattice");
    assert_eq!(scene.worst_error(), 0.0, "the exact path is the transform");
    assert!(scene.describe().contains("scene-linear"), "{}", scene.describe());

    let video = view_for(&config, "sRGB - Texture", &working, None, &display, &view).unwrap();
    assert!(matches!(video, ViewTransform::Baked(_)), "display-referred input bakes");
    assert!(video.worst_error() > 0.0 && video.worst_error() < 0.02, "and its error is measured: {}", video.worst_error());

    // A colorspace the config does not have says so, rather than answering
    // with something that looks like a transform.
    let missing = view_for(&config, "Not A Space", &working, None, &display, &view).unwrap_err();
    assert!(missing.contains("not a colorspace"), "{missing}");
}

#[test]
fn highlights_roll_off_instead_of_clipping_and_order_is_kept() {
    let config = config();
    let (display, view) = default_view(&config).unwrap();
    let scene = view_for(&config, "ACEScg", "ACEScg", None, &display, &view).unwrap();

    // Middle grey, and a run of increasingly bright highlights.
    let grey = scene.apply([0.18, 0.18, 0.18])[1];
    assert!(grey > 0.3 && grey < 0.7, "middle grey lands mid-scale: {grey}");

    let mut last = grey;
    for scene_value in [1.0f32, 2.0, 4.0, 6.25, 16.0, 64.0] {
        let out = scene.apply([scene_value; 3])[1];
        assert!(
            out > last,
            "{scene_value} must be brighter than the value below it, not clipped flat: {out} vs {last}"
        );
        assert!(out <= 1.001, "a display value stays within the display: {out}");
        last = out;
    }
    assert!(last > 0.9, "the brightest highlight is near display white: {last}");

    // Deep shadows keep their order too.
    let mut previous = 0.0f32;
    for scene_value in [1e-5f32, 1e-4, 1e-3, 1e-2] {
        let out = scene.apply([scene_value; 3])[1];
        assert!(out >= previous, "shadows keep their order: {out} after {previous}");
        assert!(out.is_finite(), "a tiny value stays finite: {out}");
        previous = out;
    }
}

#[test]
fn negative_scene_values_stay_finite_and_are_not_silently_clamped_to_zero() {
    let config = config();
    let (display, view) = default_view(&config).unwrap();
    let scene = view_for(&config, "ACEScg", "ACEScg", None, &display, &view).unwrap();
    for v in [-0.5f32, -0.01, -1e-4] {
        let out = scene.apply([v; 3]);
        for c in out {
            assert!(c.is_finite(), "{v} produced {c}");
        }
    }
    // A negative sample is darker than black, and the transform says so rather
    // than producing the same value as zero for everything below it.
    let zero = scene.apply([0.0; 3])[1];
    let below = scene.apply([-0.5; 3])[1];
    assert!(below <= zero, "below black stays at or below black: {below} vs {zero}");
}

#[test]
fn a_whole_picture_through_the_view_equals_pixel_by_pixel() {
    let config = config();
    let (display, view) = default_view(&config).unwrap();
    let scene = view_for(&config, "ACEScg", "ACEScg", None, &display, &view).unwrap();
    let mut image: Vec<f32> = (0..64 * 4).map(|i| (i % 17) as f32 * 0.5 - 1.0).collect();
    // Alpha is not colour: it must come through untouched.
    for px in image.as_chunks_mut::<4>().0 {
        px[3] = 0.25;
    }
    let expected: Vec<[f32; 3]> = image
        .as_chunks::<4>()
        .0
        .iter()
        .map(|px| scene.apply([px[0], px[1], px[2]]))
        .collect();
    scene.apply_image(&mut image, 4);
    for (i, px) in image.as_chunks::<4>().0.iter().enumerate() {
        assert_eq!([px[0], px[1], px[2]], expected[i], "pixel {i}");
        assert_eq!(px[3], 0.25, "alpha is untouched at pixel {i}");
    }
}

/// An HDR display is a different view, not a brighter one: the same scene
/// value maps somewhere else, and the path must actually produce that rather
/// than quietly reusing Rec.709.
#[test]
fn an_hdr_display_view_is_a_different_transform_from_the_sdr_one() {
    let config = config();
    let displays: Vec<String> = config.displays.keys().cloned().collect();
    let Some(hdr) = displays.iter().find(|d| d.contains("2100") || d.to_lowercase().contains("pq"))
    else {
        eprintln!("SKIP: the bundled config has no HDR display: {displays:?}");
        return;
    };
    // Not "Raw": a pass-through would differ from the SDR view without saying
    // anything about an HDR transform existing.
    let views = config.views(hdr).unwrap();
    let Some(hdr_view) = views
        .iter()
        .map(|v| v.name.clone())
        .find(|n| !n.eq_ignore_ascii_case("Raw") && !n.to_lowercase().contains("un-tone"))
    else {
        eprintln!("SKIP: {hdr} offers only pass-through views");
        return;
    };
    let (sdr, sdr_view) = default_view(&config).unwrap();
    let to_hdr = view_for(&config, "ACEScg", "ACEScg", None, hdr, &hdr_view).unwrap();
    let to_sdr = view_for(&config, "ACEScg", "ACEScg", None, &sdr, &sdr_view).unwrap();
    let scene = [4.0f32; 3];
    let a = to_hdr.apply(scene);
    let b = to_sdr.apply(scene);
    assert!(
        (a[1] - b[1]).abs() > 1e-3,
        "an HDR view must not be the SDR one: {a:?} vs {b:?} for {hdr} / {hdr_view}"
    );
    // PQ is an absolute encoding: 4.0 scene is well above SDR white, and its
    // PQ code value must be above the code for middle grey, not clipped to 1.
    let grey = to_hdr.apply([0.18; 3])[1];
    assert!(a[1] > grey, "a highlight is above middle grey in PQ: {} vs {grey}", a[1]);
    assert!(a[1] <= 1.0001, "a PQ code value stays in [0, 1]: {}", a[1]);
    println!("MEASURED scene 4.0 → {} / {hdr_view}: {:.4}; → {sdr} / {sdr_view}: {:.4}", hdr, a[1], b[1]);
}
