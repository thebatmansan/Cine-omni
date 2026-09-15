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
fn a_released_aces_view_compiles_as_what_it_says() {
    let c = load("cg-config-v4.0.0_aces-v2.0_ocio-v2.5.ocio");
    let p = c
        .display_processor(
            "ACEScg",
            "sRGB - Display",
            "ACES 2.0 - SDR 100 nits (Rec.709)",
        )
        .expect("the SDR Rec.709 ACES view compiles");
    assert!(p.approximations().is_empty(), "{:?}", p.ops());
    let untone = c
        .display_processor("ACEScg", "sRGB - Display", "Un-tone-mapped")
        .expect("the colorimetric view compiles");
    assert!(untone.approximations().is_empty());
}

/// Coverage, measured against OpenColorIO rendering the same files: every
/// (display, view) pair that compiles here must produce OpenColorIO's pixels
/// (`tests/data/config-views.tsv`, from `generate_config_views.py`), and every
/// pair that does not is counted with its reason.
#[test]
fn measured_view_coverage_of_the_released_configs() {
    use std::collections::BTreeMap;
    let text = include_str!("data/config-views.tsv");
    let mut configs: BTreeMap<String, Config> = BTreeMap::new();
    let mut pairs: BTreeMap<
        (String, String, String),
        Result<cine_color::processor::Processor, String>,
    > = BTreeMap::new();
    let mut worst: BTreeMap<String, f64> = BTreeMap::new();
    let mut failures = Vec::new();
    for line in text.lines().filter(|l| !l.starts_with('#')).skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        let key = (f[0].to_string(), f[1].to_string(), f[2].to_string());
        let v: Vec<f64> = f[3..].iter().map(|s| s.parse().unwrap()).collect();
        let config = configs.entry(key.0.clone()).or_insert_with(|| load(&key.0));
        let compiled = pairs.entry(key.clone()).or_insert_with(|| {
            config
                .display_processor("ACES2065-1", &key.1, &key.2)
                .map_err(|e| e.to_string())
        });
        let Ok(p) = compiled else { continue };
        let got = p.apply_f64([v[0], v[1], v[2]]);
        // Display code values. OpenColorIO computes in 32-bit floats; near
        // black, where a display curve's slope is unbounded (x^(1/2.6) turns
        // 1e-9 of float noise into 3e-4), that noise is amplified, so within
        // 1e-3 of zero agreement is to a 10-bit code value and elsewhere to 2e-5.
        let err = (0..3)
            .map(|c| {
                let d = (got[c] - v[3 + c]).abs();
                if got[c].abs().max(v[3 + c].abs()) < 1e-3 {
                    d * 2e-5 / 1e-3
                } else {
                    d
                }
            })
            .fold(0.0, f64::max);
        let w = worst.entry(key.0.clone()).or_insert(0.0);
        *w = w.max(err);
        if err > 2e-5 || !p.approximations().is_empty() {
            failures.push(format!(
                "{key:?} {:?}: {got:?} vs OCIO {:?} ({err:.2e})",
                &v[..3],
                &v[3..]
            ));
        }
    }
    for file in configs.keys() {
        let mine: Vec<_> = pairs.iter().filter(|((f, _, _), _)| f == file).collect();
        let exact = mine.iter().filter(|(_, r)| r.is_ok()).count();
        let mut refused: BTreeMap<&str, usize> = BTreeMap::new();
        for (_, r) in &mine {
            if let Err(e) = r {
                *refused.entry(e.as_str()).or_default() += 1;
            }
        }
        println!(
            "MEASURED {file}: {} display/view pairs — {exact} render as OpenColorIO does \
             (worst code-value error {:.1e}), {} refused",
            mine.len(),
            worst.get(file).copied().unwrap_or(0.0),
            mine.len() - exact
        );
        for (why, n) in &refused {
            println!("    {n} × {why}");
        }
        assert!(exact > 0);
    }
    assert!(
        failures.is_empty(),
        "{} samples disagree:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
