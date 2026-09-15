//! The ACES configs OpenColorIO 2.5.2 ships, loaded as they are.
//!
//! `tests/data/configs/*.ocio` were serialised by PyOpenColorIO 2.5.2 from its
//! built-in `ocio://cg-config-v4.0.0_aces-v2.0_ocio-v2.5` and
//! `ocio://studio-config-v4.0.0_aces-v2.0_ocio-v2.5` (BSD-3-Clause, Academy
//! Software Foundation). Until shared views existed here, neither loaded at all.

use cine_color::ocio::Config;
use std::path::Path;

fn load(name: &str) -> Config {
    Config::load(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/configs")
            .join(name),
    )
    .unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn the_released_aces_2_configs_load_with_their_shared_views_resolved() {
    for (file, colorspaces, displays) in [
        // Counts as OpenColorIO 2.5.2 reports them for the same files.
        // All colorspaces, inactive included (OCIO: SEARCH_REFERENCE_SPACE_ALL,
        // COLORSPACE_ALL), and all displays.
        ("cg-config-v4.0.0_aces-v2.0_ocio-v2.5.ocio", 27, 8),
        ("studio-config-v4.0.0_aces-v2.0_ocio-v2.5.ocio", 57, 9),
    ] {
        let c = load(file);
        println!(
            "{file}: {} colorspaces, {} displays, unimplemented keys {:?}",
            c.colorspaces.len(),
            c.displays.len(),
            c.unimplemented
        );
        assert_eq!(c.colorspaces.len(), colorspaces, "{file}");
        assert_eq!(c.displays.len(), displays, "{file}");
        let srgb = c.views("sRGB - Display").unwrap();
        let aces = srgb
            .iter()
            .find(|v| v.name == "ACES 2.0 - SDR 100 nits (Rec.709)")
            .expect("the shared ACES view is on the sRGB display");
        assert_eq!(
            aces.display_colorspace.as_deref(),
            Some("sRGB - Display"),
            "<USE_DISPLAY_NAME> resolved"
        );
        assert!(
            c.unimplemented
                .iter()
                .any(|k| k == "named_transforms" || k == "viewing_rules"),
            "what is not applied is listed"
        );
    }
}

#[test]
fn a_released_view_compiles_and_says_what_it_approximates() {
    let c = load("cg-config-v4.0.0_aces-v2.0_ocio-v2.5.ocio");
    let p = c
        .display_processor(
            "ACEScg",
            "sRGB - Display",
            "ACES 2.0 - SDR 100 nits (Rec.709)",
        )
        .expect("the SDR Rec.709 ACES view compiles");
    let notes = p.approximations();
    assert_eq!(notes.len(), 1, "{:?}", p.ops());
    let untone = c
        .display_processor("ACEScg", "sRGB - Display", "Un-tone-mapped")
        .expect("the colorimetric view compiles");
    assert!(untone.approximations().is_empty());
}

/// Coverage, measured: of every (display, view) pair in the released configs,
/// how many compile, how many of those are approximations, and what refuses.
#[test]
fn measured_view_coverage_of_the_released_configs() {
    for file in [
        "cg-config-v4.0.0_aces-v2.0_ocio-v2.5.ocio",
        "studio-config-v4.0.0_aces-v2.0_ocio-v2.5.ocio",
    ] {
        let c = load(file);
        let (mut exact, mut approximate) = (0, 0);
        let mut refused: std::collections::BTreeMap<String, usize> = Default::default();
        for (display, views) in &c.displays {
            for v in views {
                match c.display_processor("ACES2065-1", display, &v.name) {
                    Ok(p) if p.approximations().is_empty() => exact += 1,
                    Ok(_) => approximate += 1,
                    Err(e) => *refused.entry(e.to_string()).or_default() += 1,
                }
            }
        }
        let total = exact + approximate + refused.values().sum::<usize>();
        println!(
            "MEASURED {file}: {total} display/view pairs — {exact} compile as stated, \
             {approximate} compile as marked approximations, {} refused",
            refused.values().sum::<usize>()
        );
        for (why, n) in &refused {
            println!("    {n} × {why}");
        }
        assert!(exact + approximate > 0);
    }
}
