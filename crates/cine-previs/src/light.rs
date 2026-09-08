//! Light physics: blackbody colour, photometric units and fixture presets.
//!
//! * Colour temperature → linear RGB uses the CIE 1931 `xy` Planckian-locus
//!   fit of Kim et al. / Kang et al. (2002), then `xy → XYZ → linear sRGB
//!   (Rec.709 primaries, D65)`. Results are white balanced so that a 6500 K
//!   source is exactly `(1, 1, 1)`.
//! * Photometry follows the usual film-set conventions: fixtures are rated in
//!   lumens (total flux), the set is measured in lux (illuminance), and the
//!   difference between two readings is expressed in stops.

use serde::{Deserialize, Serialize};
use ucg_schema::{LightParams, LightType};

// ---------------------------------------------------------------------------
// Blackbody colour
// ---------------------------------------------------------------------------

/// Lowest colour temperature the locus fit is valid for.
pub const KELVIN_MIN: f64 = 1667.0;
/// Highest colour temperature the locus fit is valid for.
pub const KELVIN_MAX: f64 = 25000.0;
/// The reference white (a 6500 K blackbody) that [`kelvin_to_rgb`] maps to `(1,1,1)`.
pub const REFERENCE_WHITE_K: f64 = 6500.0;

/// CIE 1931 chromaticity `(x, y)` of a Planckian radiator at `kelvin`
/// (clamped to `1667..=25000`). Kang et al. 2002 cubic fit.
pub fn kelvin_to_xy(kelvin: f64) -> [f64; 2] {
    let t = kelvin.clamp(KELVIN_MIN, KELVIN_MAX);
    let t2 = t * t;
    let t3 = t2 * t;
    let x = if t <= 4000.0 {
        -0.2661239e9 / t3 - 0.2343589e6 / t2 + 0.8776956e3 / t + 0.179910
    } else {
        -3.0258469e9 / t3 + 2.1070379e6 / t2 + 0.2226347e3 / t + 0.240390
    };
    let x2 = x * x;
    let x3 = x2 * x;
    let y = if t <= 2222.0 {
        -1.1063814 * x3 - 1.34811020 * x2 + 2.18555832 * x - 0.20219683
    } else if t <= 4000.0 {
        -0.9549476 * x3 - 1.37418593 * x2 + 2.09137015 * x - 0.16748867
    } else {
        3.0817580 * x3 - 5.87338670 * x2 + 3.75112997 * x - 0.37001483
    };
    [x, y]
}

/// `xy` chromaticity → CIE XYZ with `Y = 1`.
pub fn xy_to_xyz(xy: [f64; 2]) -> [f64; 3] {
    let [x, y] = xy;
    if y.abs() < 1e-12 {
        return [0.0, 0.0, 0.0];
    }
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// CIE XYZ (D65) → linear sRGB / Rec.709 (IEC 61966-2-1 matrix).
pub fn xyz_to_linear_srgb(xyz: [f64; 3]) -> [f64; 3] {
    let [x, y, z] = xyz;
    [
        3.2406 * x - 1.5372 * y - 0.4986 * z,
        -0.9689 * x + 1.8758 * y + 0.0415 * z,
        0.0557 * x - 0.2040 * y + 1.0570 * z,
    ]
}

/// Unnormalized linear RGB of a blackbody at `kelvin` (luminance `Y = 1`).
pub fn kelvin_to_rgb_raw(kelvin: f64) -> [f64; 3] {
    let rgb = xyz_to_linear_srgb(xy_to_xyz(kelvin_to_xy(kelvin)));
    rgb.map(|c| c.max(0.0))
}

/// Apparent linear RGB of a `kelvin` source seen by a camera white balanced
/// to `white_balance_k` (per-channel von Kries scaling, `Y ≈ 1`).
pub fn kelvin_to_rgb_balanced(kelvin: f64, white_balance_k: f64) -> [f64; 3] {
    let src = kelvin_to_rgb_raw(kelvin);
    let wb = kelvin_to_rgb_raw(white_balance_k);
    [
        if wb[0] > 1e-9 { src[0] / wb[0] } else { 0.0 },
        if wb[1] > 1e-9 { src[1] / wb[1] } else { 0.0 },
        if wb[2] > 1e-9 { src[2] / wb[2] } else { 0.0 },
    ]
}

/// Linear sRGB colour of a blackbody at `kelvin`, normalized so that
/// 6500 K is exactly white. Tungsten (3200 K) comes out warm
/// (`r > g > b`), daylight-blue skies (10000 K) come out cool.
pub fn kelvin_to_rgb(kelvin: f64) -> [f64; 3] {
    kelvin_to_rgb_balanced(kelvin, REFERENCE_WHITE_K)
}

/// Final linear RGB emitted by a light: blackbody colour × tint.
pub fn light_rgb(params: &LightParams) -> [f64; 3] {
    let c = kelvin_to_rgb(params.color_temp_k);
    [
        c[0] * params.tint[0],
        c[1] * params.tint[1],
        c[2] * params.tint[2],
    ]
}

/// Linear → sRGB display encoding of one channel (for swatches / UI).
pub fn linear_to_srgb(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

// ---------------------------------------------------------------------------
// Photometry
// ---------------------------------------------------------------------------

/// Solid angle (steradians) of a cone with the given full apex angle.
pub fn cone_solid_angle_sr(cone_angle_deg: f64) -> f64 {
    let half = (cone_angle_deg.clamp(0.0, 360.0) * 0.5).to_radians();
    2.0 * std::f64::consts::PI * (1.0 - half.cos())
}

/// Luminous intensity (cd) of an isotropic point source: `lm / 4π`.
pub fn point_lumens_to_candela(lumens: f64) -> f64 {
    lumens / (4.0 * std::f64::consts::PI)
}

/// Luminous intensity (cd) of a spot that spreads its flux uniformly over
/// a cone: `lm / Ω(cone)`.
pub fn spot_lumens_to_candela(lumens: f64, cone_angle_deg: f64) -> f64 {
    let sr = cone_solid_angle_sr(cone_angle_deg);
    if sr <= 1e-12 {
        f64::INFINITY
    } else {
        lumens / sr
    }
}

/// On-axis intensity (cd) of a flat Lambertian panel: `lm / π`.
pub fn area_lumens_to_candela(lumens: f64) -> f64 {
    lumens / std::f64::consts::PI
}

/// Total flux (lm) a spot needs to reach `lux_at_1m` on axis with a given cone.
pub fn lumens_for_axial_lux(lux_at_1m: f64, cone_angle_deg: f64) -> f64 {
    lux_at_1m * cone_solid_angle_sr(cone_angle_deg)
}

/// Illuminance (lux) at `distance_m` from a source of `candela` (inverse square).
pub fn illuminance_lux(candela: f64, distance_m: f64) -> f64 {
    if distance_m <= 0.0 {
        return f64::INFINITY;
    }
    candela / (distance_m * distance_m)
}

/// On-axis luminous intensity (cd) of a light. For directional/dome lights
/// (rated in lux) this returns `None` as intensity is distance independent.
pub fn candela_of(params: &LightParams) -> Option<f64> {
    match params.kind {
        LightType::Point => Some(point_lumens_to_candela(params.intensity)),
        LightType::Spot => Some(spot_lumens_to_candela(
            params.intensity,
            params.cone_angle_deg,
        )),
        LightType::Area => Some(area_lumens_to_candela(params.intensity)),
        LightType::Directional | LightType::Dome => None,
    }
}

/// On-axis illuminance (lux) produced by a light at `distance_m`.
pub fn illuminance_from_light(params: &LightParams, distance_m: f64) -> f64 {
    match candela_of(params) {
        Some(cd) => illuminance_lux(cd, distance_m),
        None => params.intensity,
    }
}

/// Incident-light exposure value at ISO 100 for a given illuminance
/// (`EV = log2(lux / 2.5)`, incident calibration constant `C = 250`).
pub fn lux_to_ev(lux: f64) -> f64 {
    (lux / 2.5).log2()
}

/// Inverse of [`lux_to_ev`].
pub fn ev_to_lux(ev: f64) -> f64 {
    2.5 * ev.exp2()
}

/// Exposure value at a given ISO for an illuminance.
pub fn lux_to_ev_at_iso(lux: f64, iso: f64) -> f64 {
    lux_to_ev(lux) + (iso / 100.0).log2()
}

/// Difference in stops from `a` to `b` (positive when `b` is brighter).
pub fn stops_between(a: f64, b: f64) -> f64 {
    (b / a).log2()
}

/// Distance (m) at which a light produces `target_lux` (inverse square).
/// `None` for distance-independent lights or unreachable targets.
pub fn distance_for_lux(params: &LightParams, target_lux: f64) -> Option<f64> {
    let cd = candela_of(params)?;
    if target_lux <= 0.0 || cd <= 0.0 {
        return None;
    }
    Some((cd / target_lux).sqrt())
}

// ---------------------------------------------------------------------------
// Fixture presets
// ---------------------------------------------------------------------------

/// Common set fixtures. All figures are **approximate** manufacturer or
/// field-measured values, rounded, intended for previs exposure estimates
/// rather than photometric reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixturePreset {
    /// ARRI SkyPanel S60-C: 645 × 300 mm RGBW soft panel, ≈ 43 000 lm at
    /// 5600 K (tunable 2800–10000 K).
    ArriSkyPanelS60C,
    /// Aputure LS 600d: 600 W daylight COB. ≈ 88 000 lux @ 1 m with the
    /// hyper reflector; modelled as a 30° cone, which implies ≈ 18 800 lm.
    AputureLs600d,
    /// Kino Flo 4Bank 4 ft: four 75 W tubes ≈ 11 000 lm, 1.22 × 0.30 m,
    /// KF55 tubes at 5500 K.
    KinoFlo4Bank4ft,
    /// 2 kW tungsten Fresnel (ARRI T2 class): ≈ 52 000 lm bare lamp at
    /// 3200 K, ≈ 50° flood beam.
    TungstenFresnel2k,
}

impl FixturePreset {
    /// Every preset in display order.
    pub const ALL: [FixturePreset; 4] = [
        FixturePreset::ArriSkyPanelS60C,
        FixturePreset::AputureLs600d,
        FixturePreset::KinoFlo4Bank4ft,
        FixturePreset::TungstenFresnel2k,
    ];

    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            FixturePreset::ArriSkyPanelS60C => "ARRI SkyPanel S60-C",
            FixturePreset::AputureLs600d => "Aputure LS 600d",
            FixturePreset::KinoFlo4Bank4ft => "Kino Flo 4Bank 4ft",
            FixturePreset::TungstenFresnel2k => "2K Tungsten Fresnel",
        }
    }

    /// Physical light parameters for the fixture.
    pub fn params(self) -> LightParams {
        match self {
            FixturePreset::ArriSkyPanelS60C => LightParams {
                kind: LightType::Area,
                intensity: 43_000.0,
                color_temp_k: 5600.0,
                tint: [1.0; 3],
                cone_angle_deg: 115.0,
                softness: 0.8,
                size_m: [0.645, 0.30],
            },
            FixturePreset::AputureLs600d => LightParams {
                kind: LightType::Spot,
                intensity: lumens_for_axial_lux(88_000.0, 30.0).round(),
                color_temp_k: 5600.0,
                tint: [1.0; 3],
                cone_angle_deg: 30.0,
                softness: 0.15,
                size_m: [0.2, 0.2],
            },
            FixturePreset::KinoFlo4Bank4ft => LightParams {
                kind: LightType::Area,
                intensity: 11_000.0,
                color_temp_k: 5500.0,
                tint: [1.0; 3],
                cone_angle_deg: 120.0,
                softness: 0.9,
                size_m: [1.22, 0.30],
            },
            FixturePreset::TungstenFresnel2k => LightParams {
                kind: LightType::Spot,
                intensity: 52_000.0,
                color_temp_k: 3200.0,
                tint: [1.0; 3],
                cone_angle_deg: 50.0,
                softness: 0.3,
                size_m: [0.25, 0.25],
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_white_is_white() {
        let w = kelvin_to_rgb(6500.0);
        for c in w {
            assert!((c - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn locus_is_close_to_d65() {
        let [x, y] = kelvin_to_xy(6500.0);
        assert!((x - 0.3135).abs() < 0.003);
        assert!((y - 0.3237).abs() < 0.003);
    }

    #[test]
    fn warm_is_red_cool_is_blue() {
        let t = kelvin_to_rgb(3200.0);
        assert!(t[0] > t[1] && t[1] > t[2]);
        let d = kelvin_to_rgb(10000.0);
        assert!(d[2] > d[0]);
        // Red/blue ratio strictly decreasing with temperature.
        let mut prev = f64::INFINITY;
        for k in (2000..=12000).step_by(250) {
            let c = kelvin_to_rgb(k as f64);
            let ratio = c[0] / c[2];
            assert!(ratio < prev, "ratio not monotone at {k}K");
            prev = ratio;
        }
    }

    #[test]
    fn balanced_camera_sees_its_own_white() {
        let c = kelvin_to_rgb_balanced(3200.0, 3200.0);
        for ch in c {
            assert!((ch - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn photometry_basics() {
        assert!((illuminance_lux(point_lumens_to_candela(1000.0), 1.0) - 79.577).abs() < 0.01);
        assert!((illuminance_lux(point_lumens_to_candela(1000.0), 2.0) - 19.894).abs() < 0.01);
        assert!((cone_solid_angle_sr(360.0) - 4.0 * std::f64::consts::PI).abs() < 1e-9);
        assert!((stops_between(100.0, 400.0) - 2.0).abs() < 1e-12);
        assert!((lux_to_ev(2.5)).abs() < 1e-12);
        assert!((lux_to_ev(2560.0) - 10.0).abs() < 1e-9);
        assert!((ev_to_lux(lux_to_ev(1234.0)) - 1234.0).abs() < 1e-9);
        assert!((lux_to_ev_at_iso(2.5, 800.0) - 3.0).abs() < 1e-12);
        let lm = lumens_for_axial_lux(88_000.0, 30.0);
        assert!((spot_lumens_to_candela(lm, 30.0) - 88_000.0).abs() < 1e-6);
    }

    #[test]
    fn fixtures_are_sane() {
        for f in FixturePreset::ALL {
            let p = f.params();
            assert!(p.intensity > 0.0, "{}", f.name());
            assert!(p.color_temp_k >= 2800.0 && p.color_temp_k <= 6500.0);
        }
        let d600 = FixturePreset::AputureLs600d.params();
        assert!((illuminance_from_light(&d600, 1.0) - 88_000.0).abs() < 5.0);
        assert!(distance_for_lux(&d600, 1000.0).unwrap() > 9.0);
        let sun = LightParams {
            kind: LightType::Directional,
            intensity: 100_000.0,
            ..Default::default()
        };
        assert_eq!(illuminance_from_light(&sun, 50.0), 100_000.0);
        assert!(distance_for_lux(&sun, 10.0).is_none());
    }

    #[test]
    fn srgb_encoding_endpoints() {
        assert_eq!(linear_to_srgb(0.0), 0.0);
        assert!((linear_to_srgb(1.0) - 1.0).abs() < 1e-12);
        assert!((linear_to_srgb(0.18) - 0.461).abs() < 0.002);
    }
}
