//! Transfer functions (opto-electronic encodings) with exact published formulas.
//!
//! `encode` maps linear light to a code value, `decode` is the exact inverse.
//! Linear values are relative: for camera log curves 0.18 is 18 % scene gray,
//! for PQ 1.0 is 10 000 cd/m², for HLG 1.0 is the nominal peak.

use serde::{Deserialize, Serialize};

/// A transfer function.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transfer {
    /// Identity.
    Linear,
    /// IEC 61966-2-1 sRGB piecewise curve.
    SRgb,
    /// Pure power law `code = lin^(1/gamma)` (2.2, 2.4 = Rec.1886, 2.6 = DCI).
    Gamma(f64),
    /// ITU-R BT.709 camera OETF.
    Rec709,
    /// SMPTE ST 2084 perceptual quantizer (1.0 = 10 000 nits).
    Pq,
    /// ITU-R BT.2100 hybrid log-gamma OETF (1.0 = nominal peak).
    Hlg,
    /// ARRI LogC3 (ALEXA classic) at EI 800.
    LogC3,
    /// ARRI LogC4 (ALEXA 35).
    LogC4,
    /// Sony S-Log3.
    SLog3,
    /// RED Log3G10 (v3).
    Log3G10,
    /// Canon Log 2 (10-bit code value / 1023; 18 % gray → 0.398).
    CanonLog2,
    /// Canon Log 3 (10-bit code value / 1023; 18 % gray → 0.343).
    CanonLog3,
    /// Panasonic V-Log.
    VLog,
    /// ACEScc logarithmic encoding.
    ACEScc,
    /// ACEScct logarithmic encoding with a linear toe.
    ACEScct,
}

// --- constants --------------------------------------------------------------

const PQ_M1: f64 = 2610.0 / 16384.0;
const PQ_M2: f64 = 2523.0 / 4096.0 * 128.0;
const PQ_C1: f64 = 3424.0 / 4096.0;
const PQ_C2: f64 = 2413.0 / 4096.0 * 32.0;
const PQ_C3: f64 = 2392.0 / 4096.0 * 32.0;

const HLG_A: f64 = 0.17883277;
const HLG_B: f64 = 1.0 - 4.0 * HLG_A;
// c = 0.5 - a * ln(4a)
const HLG_C: f64 = 0.559_910_729_529_562_4;

// LogC3 EI800
const LOGC3_CUT: f64 = 0.010591;
const LOGC3_A: f64 = 5.555556;
const LOGC3_B: f64 = 0.052272;
const LOGC3_C: f64 = 0.247190;
const LOGC3_D: f64 = 0.385537;
const LOGC3_E: f64 = 5.367655;
const LOGC3_F: f64 = 0.092809;

// LogC4
const LOGC4_A: f64 = (262_144.0 - 16.0) / 117.45;
const LOGC4_B: f64 = (1023.0 - 95.0) / 1023.0;
const LOGC4_C: f64 = 95.0 / 1023.0;

// S-Log3
const SLOG3_CUT: f64 = 0.01125;
const SLOG3_LIN_TOP: f64 = 171.2102946929;

// Log3G10 v3
const L3G10_A: f64 = 0.224282;
const L3G10_B: f64 = 155.975327;
const L3G10_C: f64 = 0.01;
const L3G10_G: f64 = 15.1927;

// Canon Log 2 / 3 (Canon white paper "Canon Log Gamma Curves", 10-bit code
// value form: `code = CV / 1023`). Canon's curves take linear light with
// 90 % reflectance at 1.0, so 18 % gray enters the formulas as 0.2; the crate
// keeps the usual "18 % gray = 0.18" convention and rescales by 0.9.
const CANON_REFLECTANCE: f64 = 0.9;
const CLOG2_A: f64 = 0.24136077;
const CLOG2_B: f64 = 87.09937546;
const CLOG2_C: f64 = 0.092864125;
const CLOG3_A: f64 = 0.36726845;
const CLOG3_B: f64 = 14.98325;
/// Offset of the negative log branch of Canon Log 3.
const CLOG3_C_NEG: f64 = 0.12783901;
/// Offset of the positive log branch of Canon Log 3.
const CLOG3_C_POS: f64 = 0.12240537;
const CLOG3_LIN_SLOPE: f64 = 1.9754798;
const CLOG3_LIN_OFF: f64 = 0.12512219;
/// Linear segment of Canon Log 3 spans `[-CLOG3_LIN_BREAK, CLOG3_LIN_BREAK]` (Canon units).
const CLOG3_LIN_BREAK: f64 = 0.014;

// V-Log
const VLOG_CUT1: f64 = 0.01;
const VLOG_CUT2: f64 = 0.181;
const VLOG_B: f64 = 0.00873;
const VLOG_C: f64 = 0.241514;
const VLOG_D: f64 = 0.598206;

// ACEScct
const CCT_X_BRK: f64 = 0.0078125;
const CCT_Y_BRK: f64 = 0.155251141552511;
const CCT_A: f64 = 10.5402377416545;
const CCT_B: f64 = 0.0729055341958355;

fn sign_pow(x: f64, p: f64) -> f64 {
    if x < 0.0 {
        -((-x).powf(p))
    } else {
        x.powf(p)
    }
}

impl Transfer {
    /// Linear → code value.
    pub fn encode(&self, x: f64) -> f64 {
        match self {
            Transfer::Linear => x,
            Transfer::SRgb => {
                if x <= 0.0031308 {
                    12.92 * x
                } else {
                    1.055 * x.powf(1.0 / 2.4) - 0.055
                }
            }
            Transfer::Gamma(g) => sign_pow(x, 1.0 / g),
            Transfer::Rec709 => {
                if x < 0.018 {
                    4.5 * x
                } else {
                    1.099 * x.powf(0.45) - 0.099
                }
            }
            Transfer::Pq => {
                let y = x.max(0.0).powf(PQ_M1);
                ((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y)).powf(PQ_M2)
            }
            Transfer::Hlg => {
                let e = x.max(0.0);
                if e <= 1.0 / 12.0 {
                    (3.0 * e).sqrt()
                } else {
                    HLG_A * (12.0 * e - HLG_B).ln() + HLG_C
                }
            }
            Transfer::LogC3 => {
                if x > LOGC3_CUT {
                    LOGC3_C * (LOGC3_A * x + LOGC3_B).log10() + LOGC3_D
                } else {
                    LOGC3_E * x + LOGC3_F
                }
            }
            Transfer::LogC4 => {
                let (s, t) = logc4_st();
                if x < t {
                    (x - t) / s
                } else {
                    ((LOGC4_A * x + 64.0).log2() - 6.0) / 14.0 * LOGC4_B + LOGC4_C
                }
            }
            Transfer::SLog3 => {
                if x >= SLOG3_CUT {
                    (420.0 + ((x + 0.01) / (0.18 + 0.01)).log10() * 261.5) / 1023.0
                } else {
                    (x * (SLOG3_LIN_TOP - 95.0) / SLOG3_CUT + 95.0) / 1023.0
                }
            }
            Transfer::Log3G10 => {
                let v = x + L3G10_C;
                if v < 0.0 {
                    v * L3G10_G
                } else {
                    L3G10_A * (v * L3G10_B + 1.0).log10()
                }
            }
            Transfer::CanonLog2 => {
                let x = x / CANON_REFLECTANCE;
                if x < 0.0 {
                    -(CLOG2_A * (-x * CLOG2_B + 1.0).log10()) + CLOG2_C
                } else {
                    CLOG2_A * (x * CLOG2_B + 1.0).log10() + CLOG2_C
                }
            }
            Transfer::CanonLog3 => {
                let x = x / CANON_REFLECTANCE;
                if x < -CLOG3_LIN_BREAK {
                    -(CLOG3_A * (-x * CLOG3_B + 1.0).log10()) + CLOG3_C_NEG
                } else if x <= CLOG3_LIN_BREAK {
                    CLOG3_LIN_SLOPE * x + CLOG3_LIN_OFF
                } else {
                    CLOG3_A * (x * CLOG3_B + 1.0).log10() + CLOG3_C_POS
                }
            }
            Transfer::VLog => {
                if x < VLOG_CUT1 {
                    5.6 * x + 0.125
                } else {
                    VLOG_C * (x + VLOG_B).log10() + VLOG_D
                }
            }
            Transfer::ACEScc => {
                if x <= 0.0 {
                    ((-16.0f64) + 9.72) / 17.52
                } else if x < 2f64.powi(-15) {
                    ((2f64.powi(-16) + x * 0.5).log2() + 9.72) / 17.52
                } else {
                    (x.log2() + 9.72) / 17.52
                }
            }
            Transfer::ACEScct => {
                if x <= CCT_X_BRK {
                    CCT_A * x + CCT_B
                } else {
                    (x.log2() + 9.72) / 17.52
                }
            }
        }
    }

    /// Code value → linear.
    pub fn decode(&self, t: f64) -> f64 {
        match self {
            Transfer::Linear => t,
            Transfer::SRgb => {
                if t <= 0.04045 {
                    t / 12.92
                } else {
                    ((t + 0.055) / 1.055).powf(2.4)
                }
            }
            Transfer::Gamma(g) => sign_pow(t, *g),
            Transfer::Rec709 => {
                if t < 0.081 {
                    t / 4.5
                } else {
                    ((t + 0.099) / 1.099).powf(1.0 / 0.45)
                }
            }
            Transfer::Pq => {
                let np = t.max(0.0).powf(1.0 / PQ_M2);
                let num = (np - PQ_C1).max(0.0);
                let den = PQ_C2 - PQ_C3 * np;
                if den <= 0.0 {
                    return 1.0;
                }
                (num / den).powf(1.0 / PQ_M1)
            }
            Transfer::Hlg => {
                let e = t.max(0.0);
                if e <= 0.5 {
                    e * e / 3.0
                } else {
                    (((e - HLG_C) / HLG_A).exp() + HLG_B) / 12.0
                }
            }
            Transfer::LogC3 => {
                if t > LOGC3_E * LOGC3_CUT + LOGC3_F {
                    (10f64.powf((t - LOGC3_D) / LOGC3_C) - LOGC3_B) / LOGC3_A
                } else {
                    (t - LOGC3_F) / LOGC3_E
                }
            }
            Transfer::LogC4 => {
                let (s, tt) = logc4_st();
                if t < 0.0 {
                    t * s + tt
                } else {
                    (2f64.powf(14.0 * (t - LOGC4_C) / LOGC4_B + 6.0) - 64.0) / LOGC4_A
                }
            }
            Transfer::SLog3 => {
                if t >= SLOG3_LIN_TOP / 1023.0 {
                    10f64.powf((t * 1023.0 - 420.0) / 261.5) * (0.18 + 0.01) - 0.01
                } else {
                    (t * 1023.0 - 95.0) * SLOG3_CUT / (SLOG3_LIN_TOP - 95.0)
                }
            }
            Transfer::Log3G10 => {
                let v = if t < 0.0 {
                    t / L3G10_G
                } else {
                    (10f64.powf(t / L3G10_A) - 1.0) / L3G10_B
                };
                v - L3G10_C
            }
            Transfer::CanonLog2 => {
                let x = if t < CLOG2_C {
                    -(10f64.powf((CLOG2_C - t) / CLOG2_A) - 1.0) / CLOG2_B
                } else {
                    (10f64.powf((t - CLOG2_C) / CLOG2_A) - 1.0) / CLOG2_B
                };
                x * CANON_REFLECTANCE
            }
            Transfer::CanonLog3 => {
                let t_lo = CLOG3_LIN_OFF - CLOG3_LIN_SLOPE * CLOG3_LIN_BREAK;
                let t_hi = CLOG3_LIN_OFF + CLOG3_LIN_SLOPE * CLOG3_LIN_BREAK;
                let x = if t < t_lo {
                    -(10f64.powf((CLOG3_C_NEG - t) / CLOG3_A) - 1.0) / CLOG3_B
                } else if t <= t_hi {
                    (t - CLOG3_LIN_OFF) / CLOG3_LIN_SLOPE
                } else {
                    (10f64.powf((t - CLOG3_C_POS) / CLOG3_A) - 1.0) / CLOG3_B
                };
                x * CANON_REFLECTANCE
            }
            Transfer::VLog => {
                if t < VLOG_CUT2 {
                    (t - 0.125) / 5.6
                } else {
                    10f64.powf((t - VLOG_D) / VLOG_C) - VLOG_B
                }
            }
            Transfer::ACEScc => {
                if t <= (9.72 - 15.0) / 17.52 {
                    (2f64.powf(t * 17.52 - 9.72) - 2f64.powi(-16)) * 2.0
                } else if t < (65504f64.log2() + 9.72) / 17.52 {
                    2f64.powf(t * 17.52 - 9.72)
                } else {
                    65504.0
                }
            }
            Transfer::ACEScct => {
                if t <= CCT_Y_BRK {
                    (t - CCT_B) / CCT_A
                } else {
                    2f64.powf(t * 17.52 - 9.72)
                }
            }
        }
    }

    /// Reasonable upper bound of the linear domain (for LUT baking and tests).
    pub fn linear_peak(&self) -> f64 {
        match self {
            Transfer::Linear => 1.0,
            Transfer::SRgb
            | Transfer::Gamma(_)
            | Transfer::Rec709
            | Transfer::Pq
            | Transfer::Hlg => 1.0,
            Transfer::ACEScc | Transfer::ACEScct => 222.0,
            _ => 16.0,
        }
    }

    /// True when this is a logarithmic camera / ACES encoding.
    pub fn is_log(&self) -> bool {
        matches!(
            self,
            Transfer::LogC3
                | Transfer::LogC4
                | Transfer::SLog3
                | Transfer::Log3G10
                | Transfer::CanonLog2
                | Transfer::CanonLog3
                | Transfer::VLog
                | Transfer::ACEScc
                | Transfer::ACEScct
        )
    }

    /// Parse a name as used in OCIO configs / the UI.
    pub fn from_name(name: &str) -> Option<Transfer> {
        let n = name
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-', '_', '.'], "");
        Some(match n.as_str() {
            "linear" | "lin" => Transfer::Linear,
            "srgb" => Transfer::SRgb,
            "gamma22" | "g22" => Transfer::Gamma(2.2),
            "gamma24" | "g24" | "rec1886" | "bt1886" => Transfer::Gamma(2.4),
            "gamma26" | "g26" | "dci" => Transfer::Gamma(2.6),
            "rec709" | "bt709" => Transfer::Rec709,
            "pq" | "st2084" | "smpte2084" => Transfer::Pq,
            "hlg" | "aribstdb67" => Transfer::Hlg,
            "logc3" | "logc" | "logcei800" | "arrilogc3" => Transfer::LogC3,
            "logc4" | "arrilogc4" => Transfer::LogC4,
            "slog3" | "sonyslog3" => Transfer::SLog3,
            "log3g10" | "redlog3g10" => Transfer::Log3G10,
            "canonlog2" | "clog2" => Transfer::CanonLog2,
            "canonlog3" | "clog3" => Transfer::CanonLog3,
            "vlog" | "panasonicvlog" => Transfer::VLog,
            "acescc" => Transfer::ACEScc,
            "acescct" => Transfer::ACEScct,
            _ => return None,
        })
    }

    /// Canonical name.
    pub fn name(&self) -> String {
        match self {
            Transfer::Linear => "linear".into(),
            Transfer::SRgb => "sRGB".into(),
            Transfer::Gamma(g) => format!("gamma{g}"),
            Transfer::Rec709 => "Rec.709".into(),
            Transfer::Pq => "PQ".into(),
            Transfer::Hlg => "HLG".into(),
            Transfer::LogC3 => "LogC3".into(),
            Transfer::LogC4 => "LogC4".into(),
            Transfer::SLog3 => "S-Log3".into(),
            Transfer::Log3G10 => "Log3G10".into(),
            Transfer::CanonLog2 => "CanonLog2".into(),
            Transfer::CanonLog3 => "CanonLog3".into(),
            Transfer::VLog => "V-Log".into(),
            Transfer::ACEScc => "ACEScc".into(),
            Transfer::ACEScct => "ACEScct".into(),
        }
    }
}

/// LogC4 linear-segment slope `s` and breakpoint `t`.
fn logc4_st() -> (f64, f64) {
    let s = (7.0 * 2f64.ln() * 2f64.powf(7.0 - 14.0 * LOGC4_C / LOGC4_B)) / (LOGC4_A * LOGC4_B);
    let t = (2f64.powf(14.0 * (-LOGC4_C / LOGC4_B) + 6.0) - 64.0) / LOGC4_A;
    (s, t)
}

// --- HLG system gamma -------------------------------------------------------

/// BT.2100 HLG reference system gamma for a display of `peak_nits`
/// (`1.2` at 1000 nits, extended formula elsewhere).
pub fn hlg_system_gamma(peak_nits: f64) -> f64 {
    1.2 + 0.42 * (peak_nits / 1000.0).log10()
}

/// HLG OOTF: scene-linear normalized RGB → display light in nits.
pub fn hlg_ootf(rgb: [f64; 3], peak_nits: f64, gamma: f64) -> [f64; 3] {
    let ys = 0.2627 * rgb[0] + 0.6780 * rgb[1] + 0.0593 * rgb[2];
    let k = peak_nits * ys.max(0.0).powf(gamma - 1.0);
    [k * rgb[0], k * rgb[1], k * rgb[2]]
}

/// Inverse HLG OOTF: display light in nits → scene-linear normalized RGB.
pub fn hlg_inverse_ootf(nits: [f64; 3], peak_nits: f64, gamma: f64) -> [f64; 3] {
    let yd = 0.2627 * nits[0] + 0.6780 * nits[1] + 0.0593 * nits[2];
    if yd <= 0.0 {
        return [0.0, 0.0, 0.0];
    }
    let k = (yd / peak_nits).powf((1.0 - gamma) / gamma) / peak_nits;
    [k * nits[0], k * nits[1], k * nits[2]]
}

/// Full HLG EOTF: non-linear signal → display light in nits.
pub fn hlg_eotf_nits(code: [f64; 3], peak_nits: f64) -> [f64; 3] {
    let g = hlg_system_gamma(peak_nits);
    let scene = [
        Transfer::Hlg.decode(code[0]),
        Transfer::Hlg.decode(code[1]),
        Transfer::Hlg.decode(code[2]),
    ];
    hlg_ootf(scene, peak_nits, g)
}

/// Full HLG inverse EOTF: display light in nits → non-linear signal.
pub fn hlg_inverse_eotf_nits(nits: [f64; 3], peak_nits: f64) -> [f64; 3] {
    let g = hlg_system_gamma(peak_nits);
    let s = hlg_inverse_ootf(nits, peak_nits, g);
    [
        Transfer::Hlg.encode(s[0]),
        Transfer::Hlg.encode(s[1]),
        Transfer::Hlg.encode(s[2]),
    ]
}

/// PQ EOTF in nits.
pub fn pq_to_nits(code: f64) -> f64 {
    Transfer::Pq.decode(code) * 10_000.0
}

/// PQ inverse EOTF from nits.
pub fn nits_to_pq(nits: f64) -> f64 {
    Transfer::Pq.encode(nits / 10_000.0)
}

/// All transfer functions (for exhaustive tests / UI lists).
pub const ALL_TRANSFERS: &[Transfer] = &[
    Transfer::Linear,
    Transfer::SRgb,
    Transfer::Gamma(2.2),
    Transfer::Gamma(2.4),
    Transfer::Gamma(2.6),
    Transfer::Rec709,
    Transfer::Pq,
    Transfer::Hlg,
    Transfer::LogC3,
    Transfer::LogC4,
    Transfer::SLog3,
    Transfer::Log3G10,
    Transfer::CanonLog2,
    Transfer::CanonLog3,
    Transfer::VLog,
    Transfer::ACEScc,
    Transfer::ACEScct,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for t in ALL_TRANSFERS {
            let peak = t.linear_peak();
            for i in 0..=2000 {
                let x = peak * i as f64 / 2000.0;
                let back = t.decode(t.encode(x));
                let tol = if *t == Transfer::ACEScc && x == 0.0 {
                    1e-5
                } else {
                    1e-6 * peak.max(1.0)
                };
                assert!(
                    (back - x).abs() < tol,
                    "{t:?}: {x} -> {} -> {back}",
                    t.encode(x)
                );
            }
        }
    }

    #[test]
    fn known_values() {
        assert!((Transfer::SRgb.encode(0.18) - 0.4613).abs() < 1e-3);
        assert!((Transfer::SRgb.decode(0.5) - 0.2140).abs() < 1e-3);
        assert!((Transfer::ACEScct.encode(0.18) - 0.4135).abs() < 1e-4);
        assert!((Transfer::ACEScc.encode(0.18) - 0.4135).abs() < 1e-4);
        assert!((Transfer::LogC3.encode(0.18) - 0.391).abs() < 1e-3);
        assert!((Transfer::LogC4.encode(0.18) - 0.2784).abs() < 1e-3);
        assert!((Transfer::SLog3.encode(0.18) - 0.4105).abs() < 1e-3);
        assert!((Transfer::Log3G10.encode(0.18) - 0.3333).abs() < 1e-3);
        assert!((Transfer::VLog.encode(0.18) - 0.423).abs() < 1e-3);
        // Canon white paper: 18 % gray → CV 407 (Canon Log 2) / CV 351 (Canon Log 3), 10-bit.
        assert!(
            (Transfer::CanonLog2.encode(0.18) - 407.4 / 1023.0).abs() < 1e-3,
            "{}",
            Transfer::CanonLog2.encode(0.18)
        );
        assert!(
            (Transfer::CanonLog3.encode(0.18) - 351.3 / 1023.0).abs() < 1e-3,
            "{}",
            Transfer::CanonLog3.encode(0.18)
        );
        // Canon Log 3 is continuous across its linear segment.
        for brk in [-0.014 * 0.9, 0.014 * 0.9] {
            let a = Transfer::CanonLog3.encode(brk - 1e-9);
            let b = Transfer::CanonLog3.encode(brk + 1e-9);
            assert!((a - b).abs() < 1e-7, "{a} vs {b}");
        }
        assert!((Transfer::Rec709.encode(0.18) - 0.409).abs() < 1e-3);
        assert!((pq_to_nits(0.5) - 92.3).abs() < 0.5);
        assert!((Transfer::Pq.encode(0.01) - 0.508).abs() < 1e-3);
        assert!((Transfer::Hlg.encode(1.0 / 12.0) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn hlg_eotf_round_trip() {
        let nits = hlg_eotf_nits([0.75, 0.75, 0.75], 1000.0);
        let back = hlg_inverse_eotf_nits(nits, 1000.0);
        for c in back {
            assert!((c - 0.75).abs() < 1e-6);
        }
        // HLG code 0.75 ≈ 203 nits reference white on a 1000-nit display.
        assert!((nits[1] - 203.0).abs() < 5.0, "{nits:?}");
    }

    #[test]
    fn names_round_trip() {
        for t in ALL_TRANSFERS {
            assert_eq!(Transfer::from_name(&t.name()), Some(*t), "{t:?}");
        }
    }
}
