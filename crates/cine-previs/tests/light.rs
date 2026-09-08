use cine_previs::light::*;
use ucg_schema::{LightParams, LightType};

#[test]
fn kelvin_rgb_red_blue_ratio_is_monotone_decreasing() {
    let mut prev = f64::INFINITY;
    for k in (1800..=20000).step_by(200) {
        let c = kelvin_to_rgb(k as f64);
        assert!(c.iter().all(|v| v.is_finite() && *v >= 0.0));
        let ratio = c[0] / c[2].max(1e-9);
        assert!(ratio < prev, "not monotone at {k} K: {ratio} >= {prev}");
        prev = ratio;
    }
    let white = kelvin_to_rgb(6500.0);
    assert!(white.iter().all(|v| (v - 1.0).abs() < 1e-12));
    let tungsten = kelvin_to_rgb(3200.0);
    assert!(tungsten[0] > 1.0 && tungsten[2] < 1.0);
}

#[test]
fn lux_at_distance_inverse_square() {
    let cd = point_lumens_to_candela(1000.0);
    assert!((illuminance_lux(cd, 1.0) - 79.58).abs() < 0.01);
    assert!((illuminance_lux(cd, 2.0) - 19.89).abs() < 0.01);
    assert!((illuminance_lux(cd, 4.0) * 16.0 - illuminance_lux(cd, 1.0)).abs() < 1e-9);
    let spot = LightParams {
        kind: LightType::Spot,
        intensity: 1000.0,
        cone_angle_deg: 60.0,
        ..Default::default()
    };
    let point = LightParams {
        kind: LightType::Point,
        intensity: 1000.0,
        ..Default::default()
    };
    assert!(illuminance_from_light(&spot, 2.0) > illuminance_from_light(&point, 2.0));
    assert!((stops_between(illuminance_lux(cd, 2.0), illuminance_lux(cd, 1.0)) - 2.0).abs() < 1e-9);
}

#[test]
fn ev_values() {
    assert!((lux_to_ev(2.5)).abs() < 1e-12);
    assert!((lux_to_ev(80.0) - 5.0).abs() < 1e-12);
    assert!((lux_to_ev(100_000.0) - 15.29).abs() < 0.01);
    assert!((lux_to_ev_at_iso(2.5, 400.0) - 2.0).abs() < 1e-12);
    for ev in [-2.0, 0.0, 7.5, 16.0] {
        assert!((lux_to_ev(ev_to_lux(ev)) - ev).abs() < 1e-9);
    }
}

#[test]
fn fixture_presets_have_documented_figures() {
    let sky = FixturePreset::ArriSkyPanelS60C.params();
    assert_eq!(sky.kind, LightType::Area);
    assert_eq!(sky.color_temp_k, 5600.0);
    assert_eq!(sky.intensity, 43_000.0);
    let d600 = FixturePreset::AputureLs600d.params();
    assert!((illuminance_from_light(&d600, 1.0) - 88_000.0).abs() < 5.0);
    let fresnel = FixturePreset::TungstenFresnel2k.params();
    assert_eq!(fresnel.color_temp_k, 3200.0);
    let rgb = light_rgb(&fresnel);
    assert!(rgb[0] > rgb[2]);
    assert_eq!(FixturePreset::ALL.len(), 4);
    assert!(!FixturePreset::KinoFlo4Bank4ft.name().is_empty());
}
