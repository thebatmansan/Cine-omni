//! Parser + processor tests against `tests/fixtures/mini.ocio`.

use cine_color::prelude::*;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

#[test]
fn parses_mini_config() {
    let cfg = Config::load(&fixture("mini.ocio")).expect("parse mini.ocio");
    assert_eq!(cfg.version, "2");
    assert_eq!(cfg.name, "mini");
    assert!(cfg
        .description
        .starts_with("A tiny ACES-style config.\nTwo lines"));
    assert_eq!(cfg.search_path, vec!["luts", "."]);
    assert_eq!(cfg.roles.len(), 6);
    assert_eq!(cfg.role("scene_linear"), Some("ACEScg"));
    assert_eq!(cfg.role("color_timing"), Some("ACEScct"));
    assert_eq!(cfg.displays.len(), 2);
    assert_eq!(cfg.views("sRGB").unwrap().len(), 4);
    assert_eq!(cfg.views("Rec.709").unwrap().len(), 2);
    assert_eq!(cfg.active_displays, vec!["sRGB", "Rec.709"]);
    assert_eq!(cfg.active_views.len(), 4);
    assert_eq!(cfg.looks.len(), 1);
    assert_eq!(cfg.looks[0].process_space, "ACEScct");
    assert_eq!(cfg.view_transforms.len(), 2);
    assert_eq!(
        cfg.default_view_transform.as_deref(),
        Some("Un-tone-mapped")
    );
    let display: Vec<_> = cfg.colorspaces.iter().filter(|c| c.is_display).collect();
    let scene: Vec<_> = cfg.colorspaces.iter().filter(|c| !c.is_display).collect();
    assert_eq!(display.len(), 3);
    assert_eq!(scene.len(), 13);
    assert_eq!(cfg.colorspaces.len(), 16);
    // file order: the display block precedes the scene block in mini.ocio
    assert!(cfg.colorspaces[0].is_display && !cfg.colorspaces[15].is_display);
    let cg = cfg.colorspace("lin_ap1").unwrap();
    assert_eq!(cg.name, "ACEScg");
    assert!(matches!(
        cg.to_reference,
        Some(cine_color::TransformDef::Matrix { .. })
    ));
    assert!(cfg.colorspace("Raw").unwrap().isdata);
    assert_eq!(cfg.colorspace("sRGB - Display").unwrap().name, "sRGB");
}

#[test]
fn processors_from_fixture() {
    let cfg = Config::load(&fixture("mini.ocio")).unwrap();
    // matrix colorspace vs computed reference
    let p = cfg.processor("Linear Rec.709", "ACEScg").unwrap();
    let want = cine_color::space::rgb_to_rgb_matrix(&Primaries::REC709, &Primaries::ACES_AP1);
    let v = [0.2f32, 0.5, 0.8];
    let got = p.apply_rgb(&v);
    let exp = want.apply([0.2, 0.5, 0.8]);
    for i in 0..3 {
        assert!((got[i] as f64 - exp[i]).abs() < 2e-3, "{got:?} vs {exp:?}");
    }
    // inverse direction inside a group
    let back = cfg
        .processor("ACEScg", "Linear Rec.709")
        .unwrap()
        .apply_rgb(&got);
    for i in 0..3 {
        assert!((back[i] - v[i]).abs() < 1e-4, "{back:?}");
    }
    // sRGB texture decode through ExponentWithLinear + ColorSpaceTransform
    let p = cfg.processor("sRGB Texture", "Linear Rec.709").unwrap();
    assert!((p.apply_rgb(&[0.5; 3])[0] - 0.2140).abs() < 1e-3);
    let p = cfg.processor("Gamma 2.2", "Linear Rec.709").unwrap();
    assert!((p.apply_rgb(&[0.5; 3])[0] - 0.5f32.powf(2.2)).abs() < 1e-5);
    // camera log through builtins
    let code = Transfer::LogC4.encode(0.18) as f32;
    let o = cfg
        .processor("arri_logc4", "scene_linear")
        .unwrap()
        .apply_rgb(&[code; 3]);
    assert!((o[0] - 0.18).abs() < 1e-3, "{o:?}");
    let code = Transfer::SLog3.encode(0.18) as f32;
    let o = cfg
        .processor("Sony S-Log3 S-Gamut3.Cine", "ACEScg")
        .unwrap()
        .apply_rgb(&[code; 3]);
    assert!((o[1] - 0.18).abs() < 1e-3, "{o:?}");
    // LogCamera + noClamp range
    let p = cfg.processor("Generic Log", "ACES2065-1").unwrap();
    let q = cfg.processor("ACES2065-1", "Generic Log").unwrap();
    let o = q.apply_rgb(&p.apply_rgb(&[0.3, 0.6, 0.9]));
    assert!(
        (o[0] - 0.3).abs() < 1e-5 && (o[2] - 0.9).abs() < 1e-5,
        "{o:?}"
    );
    // file LUTs resolved relative to the config
    let p = cfg.processor("Identity LUT", "ACES2065-1").unwrap();
    let o = p.apply_rgb(&[0.1, 0.5, 0.9]);
    assert!(
        (o[0] - 0.1).abs() < 1e-5 && (o[2] - 0.9).abs() < 1e-5,
        "{o:?}"
    );
    let p = cfg.processor("ACES2065-1", "Shaper LUT").unwrap();
    assert!((p.apply_rgb(&[0.5; 3])[0] - 0.5f32.powf(1.0 / 2.2)).abs() < 2e-3);
    assert!(matches!(
        cfg.processor("Shaper LUT", "ACES2065-1"),
        Err(Error::Unsupported(_))
    ));
    // display views
    let srgb = cfg
        .display_processor("ACEScg", "sRGB", "Un-tone-mapped")
        .unwrap();
    let o = srgb.apply_rgb(&[0.18; 3]);
    assert!((o[0] - 0.4613).abs() < 3e-3, "{o:?}");
    let toned = cfg
        .display_processor("ACEScg", "sRGB", "ACES SDR")
        .unwrap()
        .apply_rgb(&[0.18; 3]);
    assert!(toned[0] > 0.3 && toned[0] < 0.5, "{toned:?}");
    let warm = cfg
        .display_processor("ACEScg", "sRGB", "Warm")
        .unwrap()
        .apply_rgb(&[0.18; 3]);
    assert!(warm[0] > warm[2], "look should warm the image: {warm:?}");
    assert!(cfg
        .display_processor("ACEScg", "Rec.709", "Raw")
        .unwrap()
        .is_identity());
    assert!(matches!(
        cfg.display_processor("ACEScg", "Nope", "Raw"),
        Err(Error::UnknownDisplay(_))
    ));
    // gray ramp through the fixture's sRGB is monotone
    let p = cfg.processor("ACEScct", "sRGB").unwrap();
    let mut last = -1.0;
    for i in 0..=50 {
        let x = 0.1 + 0.45 * i as f32 / 50.0;
        let v = p.apply_rgb(&[x; 3])[0];
        assert!(v > last);
        last = v;
    }
}

#[test]
fn fixture_round_trips_through_writer() {
    let cfg = Config::load(&fixture("mini.ocio")).unwrap();
    let text = cfg.to_yaml();
    let mut back = Config::parse(&text).unwrap();
    back.base_dir = cfg.base_dir.clone();
    assert_eq!(back, cfg);
}

#[test]
fn cube_fixtures_parse() {
    let lut = cine_color::lut::load_cube(&fixture("identity.cube")).unwrap();
    let l3 = lut.as_3d().unwrap();
    assert_eq!(l3.size, 5);
    for v in [[0.0, 0.0, 0.0], [0.33, 0.66, 0.99], [1.0, 0.5, 0.25]] {
        let o = l3.apply(v);
        for i in 0..3 {
            assert!((o[i] - v[i]).abs() < 1e-6);
        }
        let t = l3.apply_with(v, Interpolation::Trilinear);
        for i in 0..3 {
            assert!((t[i] - v[i]).abs() < 1e-6);
        }
    }
    let shaper = cine_color::lut::load_cube(&fixture("gamma_shaper.cube")).unwrap();
    match shaper {
        Lut::OneD(l) => {
            assert_eq!(l.size, 33);
            assert!((l.apply([0.25; 3])[0] - 0.25f32.powf(1.0 / 2.2)).abs() < 5e-3);
        }
        _ => panic!("expected 1D"),
    }
}
