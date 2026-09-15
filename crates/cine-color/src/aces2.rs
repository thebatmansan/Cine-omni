//! The ACES 2.0 output transform, ported from the Academy's reference.
//!
//! Source: `aces-core` (`https://github.com/aces-aswf/aces-core`), commit
//! `069b0bc3e1f6c62820f19fdae2fecec3f4fc0f80`, files
//! `lib/Lib.Academy.OutputTransform.ctl`, `Lib.Academy.Tonescale.ctl` and
//! `Lib.Academy.Utilities.ctl`. SPDX-License-Identifier: Apache-2.0.
//! Copyright Contributors to the ACES Project. Ported to Rust for CINE-OMNI;
//! the structure, names and constants follow the CTL so the two can be read
//! side by side, and every deviation from a literal transcription is marked.
//!
//! ## Conventions carried over from CTL
//!
//! * Matrices are **row-vector**: `mul_v_m(v, M)` is CTL's `mult_f3_f33(v, M)`,
//!   `v · M`. `rgb_to_xyz` is CTL's `RGBtoXYZ_f33`, the transpose of the usual
//!   column-vector matrix.
//! * `copysign(x, 0)` is 0, as CTL defines it (`sign(y) * fabs(x)`).
//! * Float-to-int conversions truncate toward zero.
//!
//! ## What this module is checked against
//!
//! OpenColorIO 2.5.2's ACES 2.0 output transforms (`tests/aces2_oracle.rs`).

use std::f64::consts::PI;

type M3 = [[f64; 3]; 3];
type V3 = [f64; 3];

/// Primaries and white as CIE xy, in the order red, green, blue, white.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chromaticities {
    pub red: [f64; 2],
    pub green: [f64; 2],
    pub blue: [f64; 2],
    pub white: [f64; 2],
}

pub const AP0: Chromaticities = Chromaticities {
    red: [0.73470, 0.26530],
    green: [0.00000, 1.00000],
    blue: [0.00010, -0.07700],
    white: [0.32168, 0.33767],
};
pub const AP1: Chromaticities = Chromaticities {
    red: [0.713, 0.293],
    green: [0.165, 0.830],
    blue: [0.128, 0.044],
    white: [0.32168, 0.33767],
};
pub const REC709: Chromaticities = Chromaticities {
    red: [0.64, 0.33],
    green: [0.30, 0.60],
    blue: [0.15, 0.06],
    white: [0.3127, 0.3290],
};
pub const P3_D65: Chromaticities = Chromaticities {
    red: [0.680, 0.320],
    green: [0.265, 0.690],
    blue: [0.150, 0.060],
    white: [0.3127, 0.3290],
};
pub const REC2020: Chromaticities = Chromaticities {
    red: [0.708, 0.292],
    green: [0.170, 0.797],
    blue: [0.131, 0.046],
    white: [0.3127, 0.3290],
};

// ---- constants (Lib.Academy.OutputTransform.ctl) ----------------------------

const TABLE_SIZE: usize = 360;
const TOTAL_TABLE_SIZE: usize = TABLE_SIZE + 2;
const BASE_INDEX: usize = 1;
const HUE_LIMIT: f64 = 360.0;
const CUSP_CORNER_COUNT: usize = 6;
const TOTAL_CORNER_COUNT: usize = CUSP_CORNER_COUNT + 2;
const MAX_SORTED_CORNERS: usize = 2 * CUSP_CORNER_COUNT;
const REACH_CUSP_TOLERANCE: f64 = 1e-3;
const DISPLAY_CUSP_TOLERANCE: f64 = 1e-7;
const GAMMA_MINIMUM: f64 = 0.0;
const GAMMA_MAXIMUM: f64 = 5.0;
const GAMMA_SEARCH_STEP: f64 = 0.4;
const GAMMA_ACCURACY: f64 = 1e-5;

const REF_LUMINANCE: f64 = 100.0;
const L_A: f64 = 100.0;
const Y_B: f64 = 20.0;
const SURROUND: V3 = [0.9, 0.59, 0.9];
const J_SCALE: f64 = 100.0;
const CAM_NL_Y_REFERENCE: f64 = 100.0;
const CAM_NL_OFFSET: f64 = 0.2713 * CAM_NL_Y_REFERENCE;
const CAM_NL_SCALE: f64 = 4.0 * CAM_NL_Y_REFERENCE;

fn model_gamma() -> f64 {
    SURROUND[1] * (1.48 + (Y_B / REF_LUMINANCE).sqrt())
}

const CHROMA_COMPRESS: f64 = 2.4;
const CHROMA_COMPRESS_FACT: f64 = 3.3;
const CHROMA_EXPAND: f64 = 1.3;
const CHROMA_EXPAND_FACT: f64 = 0.69;
const CHROMA_EXPAND_THR: f64 = 0.5;

const SMOOTH_CUSPS: f64 = 0.12;
const SMOOTH_M: f64 = 0.27;
const CUSP_MID_BLEND: f64 = 1.3;
const FOCUS_GAIN_BLEND: f64 = 0.3;
const FOCUS_DISTANCE: f64 = 1.35;
const FOCUS_DISTANCE_SCALING: f64 = 1.75;
const COMPRESSION_THRESHOLD: f64 = 0.75;

const IDENTITY: M3 = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];

// ---- utilities (Lib.Academy.Utilities.ctl) ----------------------------------

fn mul_v_m(v: V3, m: &M3) -> V3 {
    [
        v[0] * m[0][0] + v[1] * m[1][0] + v[2] * m[2][0],
        v[0] * m[0][1] + v[1] * m[1][1] + v[2] * m[2][1],
        v[0] * m[0][2] + v[1] * m[1][2] + v[2] * m[2][2],
    ]
}

fn mul_m_m(a: &M3, b: &M3) -> M3 {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn mul_f_m(f: f64, m: &M3) -> M3 {
    m.map(|row| row.map(|v| v * f))
}

fn invert(m: &M3) -> M3 {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let inv = 1.0 / det;
    [
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv,
        ],
    ]
}

/// CTL `RGBtoXYZ_f33` (row-vector).
pub fn rgb_to_xyz(c: &Chromaticities, y: f64) -> M3 {
    let x = c.white[0] * y / c.white[1];
    let z = (1.0 - c.white[0] - c.white[1]) * y / c.white[1];
    let d = c.red[0] * (c.blue[1] - c.green[1])
        + c.blue[0] * (c.green[1] - c.red[1])
        + c.green[0] * (c.red[1] - c.blue[1]);
    let sr = (x * (c.blue[1] - c.green[1])
        - c.green[0] * (y * (c.blue[1] - 1.0) + c.blue[1] * (x + z))
        + c.blue[0] * (y * (c.green[1] - 1.0) + c.green[1] * (x + z)))
        / d;
    let sg = (x * (c.red[1] - c.blue[1])
        + c.red[0] * (y * (c.blue[1] - 1.0) + c.blue[1] * (x + z))
        - c.blue[0] * (y * (c.red[1] - 1.0) + c.red[1] * (x + z)))
        / d;
    let sb = (x * (c.green[1] - c.red[1])
        - c.red[0] * (y * (c.green[1] - 1.0) + c.green[1] * (x + z))
        + c.green[0] * (y * (c.red[1] - 1.0) + c.red[1] * (x + z)))
        / d;
    [
        [
            sr * c.red[0],
            sr * c.red[1],
            sr * (1.0 - c.red[0] - c.red[1]),
        ],
        [
            sg * c.green[0],
            sg * c.green[1],
            sg * (1.0 - c.green[0] - c.green[1]),
        ],
        [
            sb * c.blue[0],
            sb * c.blue[1],
            sb * (1.0 - c.blue[0] - c.blue[1]),
        ],
    ]
}

fn xyz_to_rgb(c: &Chromaticities, y: f64) -> M3 {
    invert(&rgb_to_xyz(c, y))
}

fn ctl_sign(x: f64) -> f64 {
    if x < 0.0 {
        -1.0
    } else if x > 0.0 {
        1.0
    } else {
        0.0
    }
}

fn copysign(x: f64, y: f64) -> f64 {
    ctl_sign(y) * x.abs()
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + t * (b - a)
}

fn lerp_v(a: V3, b: V3, t: f64) -> V3 {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
    ]
}

fn ctl_max(a: f64, b: f64) -> f64 {
    if a > b {
        a
    } else {
        b
    }
}

fn ctl_min(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else {
        b
    }
}

/// CTL `round`: half away from zero, then truncated to int.
fn ctl_round(x: f64) -> i64 {
    if x < 0.0 {
        (x - 0.5) as i64
    } else {
        (x + 0.5) as i64
    }
}

// ---- tonescale (Lib.Academy.Tonescale.ctl) ----------------------------------

#[derive(Clone, Copy, Debug)]
pub struct TsParams {
    pub n: f64,
    pub n_r: f64,
    pub g: f64,
    pub t_1: f64,
    pub c_t: f64,
    pub s_2: f64,
    pub u_2: f64,
    pub m_2: f64,
    pub forward_limit: f64,
    pub inverse_limit: f64,
    pub log_peak: f64,
}

pub fn init_ts_params(peak_luminance: f64) -> TsParams {
    let n = peak_luminance;
    let n_r = 100.0;
    let g = 1.15;
    let c = 0.18;
    let c_d = 10.013;
    let w_g = 0.14;
    let t_1 = 0.04;
    let r_hit_min = 128.0;
    let r_hit_max = 896.0;
    let r_hit = r_hit_min + (r_hit_max - r_hit_min) * ((n / n_r).ln() / (10000.0f64 / 100.0).ln());
    let m_0 = n / n_r;
    let m_1 = 0.5 * (m_0 + (m_0 * (m_0 + 4.0 * t_1)).sqrt());
    let u = ((r_hit / m_1) / ((r_hit / m_1) + 1.0)).powf(g);
    let m = m_1 / u;
    let w_i = (n / 100.0).ln() / 2.0f64.ln();
    let c_t = c_d / n_r * (1.0 + w_i * w_g);
    let g_ip = 0.5 * (c_t + (c_t * (c_t + 4.0 * t_1)).sqrt());
    let g_ipp2 = -(m_1 * (g_ip / m).powf(1.0 / g)) / ((g_ip / m).powf(1.0 / g) - 1.0);
    let w_2 = c / g_ipp2;
    let s_2 = w_2 * m_1;
    let u_2 = ((r_hit / m_1) / ((r_hit / m_1) + w_2)).powf(g);
    let m_2 = m_1 / u_2;
    TsParams {
        n,
        n_r,
        g,
        t_1,
        c_t,
        s_2,
        u_2,
        m_2,
        forward_limit: 8.0 * r_hit,
        inverse_limit: n / (u_2 * n_r),
        log_peak: (n / n_r).log10(),
    }
}

pub fn tonescale_fwd(x: f64, p: &TsParams) -> f64 {
    let f = p.m_2 * (ctl_max(0.0, x) / (x + p.s_2)).powf(p.g);
    let h = ctl_max(0.0, f * f / (f + p.t_1));
    h * p.n_r
}

pub fn tonescale_inv(y: f64, p: &TsParams) -> f64 {
    let z = ctl_max(0.0, ctl_min(p.n / (p.u_2 * p.n_r), y));
    let h = (z + (z * (4.0 * p.t_1 + z)).sqrt()) / 2.0;
    p.s_2 / ((p.m_2 / h).powf(1.0 / p.g) - 1.0)
}

// ---- CAM (Lib.Academy.OutputTransform.ctl) ----------------------------------

#[derive(Clone, Copy, Debug)]
struct JMhParams {
    rgb_to_cam16_c: M3,
    cam16_c_to_rgb: M3,
    cone_response_to_aab: M3,
    aab_to_cone_response: M3,
    f_l_n: f64,
    cz: f64,
    inv_cz: f64,
    a_w_j: f64,
    inv_a_w_j: f64,
}

fn wrap_to_360(hue: f64) -> f64 {
    let y = hue % 360.0;
    if y < 0.0 {
        y + 360.0
    } else {
        y
    }
}

fn hue_position_in_uniform_table(hue: f64, table_size: usize) -> i64 {
    (wrap_to_360(hue) / HUE_LIMIT * table_size as f64) as i64
}

fn base_hue_for_position(i_lo: i64, table_size: usize) -> f64 {
    i_lo as f64 * HUE_LIMIT / table_size as f64
}

fn pacrc_fwd_abs(rc: f64) -> f64 {
    let f_l_y = rc.powf(0.42);
    f_l_y / (CAM_NL_OFFSET + f_l_y)
}

fn pacrc_inv_abs(ra: f64) -> f64 {
    let ra_lim = ctl_min(ra, 0.99);
    let f_l_y = (CAM_NL_OFFSET * ra_lim) / (1.0 - ra_lim);
    f_l_y.powf(1.0 / 0.42)
}

fn pacrc_fwd(v: f64) -> f64 {
    copysign(pacrc_fwd_abs(v.abs()), v)
}

fn pacrc_inv(v: f64) -> f64 {
    copysign(pacrc_inv_abs(v.abs()), v)
}

fn achromatic_n_to_j(a: f64, cz: f64) -> f64 {
    J_SCALE * a.powf(cz)
}

fn j_to_achromatic_n(j: f64, inv_cz: f64) -> f64 {
    (j * (1.0 / J_SCALE)).powf(inv_cz)
}

fn a_to_y(a: f64, p: &JMhParams) -> f64 {
    pacrc_inv_abs(p.a_w_j * a) / p.f_l_n
}

fn j_to_y(j: f64, p: &JMhParams) -> f64 {
    a_to_y(j_to_achromatic_n(j.abs(), p.inv_cz), p)
}

fn y_to_j(y: f64, p: &JMhParams) -> f64 {
    let ra = pacrc_fwd_abs(y.abs() * p.f_l_n);
    let j = achromatic_n_to_j(ra * p.inv_a_w_j, p.cz);
    copysign(j, y)
}

fn rgb_to_aab(rgb: V3, p: &JMhParams) -> V3 {
    let m = mul_v_m(rgb, &p.rgb_to_cam16_c);
    let a = [pacrc_fwd(m[0]), pacrc_fwd(m[1]), pacrc_fwd(m[2])];
    mul_v_m(a, &p.cone_response_to_aab)
}

fn aab_to_jmh(aab: V3, p: &JMhParams) -> V3 {
    if aab[0] <= 0.0 {
        return [0.0, 0.0, 0.0];
    }
    let j = achromatic_n_to_j(aab[0], p.cz);
    let m = (aab[1] * aab[1] + aab[2] * aab[2]).sqrt();
    let h = wrap_to_360(aab[2].atan2(aab[1]) * 180.0 / PI);
    [j, m, h]
}

fn rgb_to_jmh(rgb: V3, p: &JMhParams) -> V3 {
    aab_to_jmh(rgb_to_aab(rgb, p), p)
}

fn jmh_to_aab(jmh: V3, p: &JMhParams) -> V3 {
    let hr = jmh[2] / 180.0 * PI;
    [
        j_to_achromatic_n(jmh[0], p.inv_cz),
        jmh[1] * hr.cos(),
        jmh[1] * hr.sin(),
    ]
}

fn aab_to_rgb(aab: V3, p: &JMhParams) -> V3 {
    let a = mul_v_m(aab, &p.aab_to_cone_response);
    let m = [pacrc_inv(a[0]), pacrc_inv(a[1]), pacrc_inv(a[2])];
    mul_v_m(m, &p.cam16_c_to_rgb)
}

fn jmh_to_rgb(jmh: V3, p: &JMhParams) -> V3 {
    aab_to_rgb(jmh_to_aab(jmh, p), p)
}

fn init_jmh_params(prims: &Chromaticities) -> JMhParams {
    let cam16_pri = Chromaticities {
        red: [0.8336, 0.1735],
        green: [2.3854, -1.4659],
        blue: [0.087, -0.125],
        white: [0.333, 0.333],
    };
    let matrix_16 = xyz_to_rgb(&cam16_pri, 1.0);
    let base_cone_response_to_aab: M3 = [
        [2.0, 1.0, 1.0 / 9.0],
        [1.0, -12.0 / 11.0, 1.0 / 9.0],
        [1.0 / 20.0, 1.0 / 11.0, -2.0 / 9.0],
    ];
    let rgb_to_xyz_m = rgb_to_xyz(prims, 1.0);
    let xyz_w = mul_v_m([REF_LUMINANCE; 3], &rgb_to_xyz_m);
    let y_w = xyz_w[1];
    let rgb_w = mul_v_m(xyz_w, &matrix_16);

    let k = 1.0 / (5.0 * L_A + 1.0);
    let k4 = k * k * k * k;
    let f_l = 0.2 * k4 * (5.0 * L_A) + 0.1 * (1.0 - k4).powi(2) * (5.0 * L_A).powf(1.0 / 3.0);
    let f_l_n = f_l / REF_LUMINANCE;
    let cz = model_gamma();

    let d_rgb = [
        f_l_n * y_w / rgb_w[0],
        f_l_n * y_w / rgb_w[1],
        f_l_n * y_w / rgb_w[2],
    ];
    let rgb_wc = [
        d_rgb[0] * rgb_w[0],
        d_rgb[1] * rgb_w[1],
        d_rgb[2] * rgb_w[2],
    ];
    let rgb_aw = [
        pacrc_fwd(rgb_wc[0]),
        pacrc_fwd(rgb_wc[1]),
        pacrc_fwd(rgb_wc[2]),
    ];

    let cone = mul_m_m(
        &mul_f_m(CAM_NL_SCALE, &IDENTITY),
        &base_cone_response_to_aab,
    );
    let a_w = cone[0][0] * rgb_aw[0] + cone[1][0] * rgb_aw[1] + cone[2][0] * rgb_aw[2];
    let a_w_j = pacrc_fwd_abs(f_l);

    let m1 = mul_m_m(&rgb_to_xyz_m, &matrix_16);
    let m2 = mul_f_m(REF_LUMINANCE, &IDENTITY);
    let rgb_to_cam16 = mul_m_m(&m1, &m2);
    let mut diag = IDENTITY;
    diag[0][0] *= d_rgb[0];
    diag[1][1] *= d_rgb[1];
    diag[2][2] *= d_rgb[2];
    let rgb_to_cam16_c = mul_m_m(&rgb_to_cam16, &diag);

    let s = 43.0 * SURROUND[2];
    let cone_response_to_aab: M3 = [
        [cone[0][0] / a_w, cone[0][1] * s, cone[0][2] * s],
        [cone[1][0] / a_w, cone[1][1] * s, cone[1][2] * s],
        [cone[2][0] / a_w, cone[2][1] * s, cone[2][2] * s],
    ];
    JMhParams {
        rgb_to_cam16_c,
        cam16_c_to_rgb: invert(&rgb_to_cam16_c),
        cone_response_to_aab,
        aab_to_cone_response: invert(&cone_response_to_aab),
        f_l_n,
        cz,
        inv_cz: 1.0 / cz,
        a_w_j,
        inv_a_w_j: 1.0 / a_w_j,
    }
}

// ---- output transform parameters --------------------------------------------

/// Everything the output transform precomputes for one display: peak luminance
/// and limiting primaries.
#[derive(Clone, Debug)]
pub struct OdtParams {
    pub peak_luminance: f64,
    input_params: JMhParams,
    reach_params: JMhParams,
    limit_params: JMhParams,
    pub ts: TsParams,
    limit_j_max: f64,
    model_gamma_inv: f64,
    table_reach_m: Vec<f64>,
    sat: f64,
    sat_thr: f64,
    compr: f64,
    chroma_compress_scale: f64,
    mid_j: f64,
    focus_dist: f64,
    lower_hull_gamma_inv: f64,
    table_hues: Vec<f64>,
    table_gamut_cusps: Vec<V3>,
    table_upper_hull_gamma: Vec<f64>,
    hue_linearity_search_range: [i64; 2],
    /// Limiting primaries' RGB to CIE XYZ (row-vector).
    limit_rgb_to_xyz: M3,
    ap0_to_ap1: M3,
    ap1_to_ap0: M3,
}

fn reach_m_from_table(h: f64, table: &[f64]) -> f64 {
    let base = hue_position_in_uniform_table(h, TABLE_SIZE);
    let t = h - base as f64;
    let i_lo = (base + BASE_INDEX as i64) as usize;
    lerp(table[i_lo], table[i_lo + 1], t)
}

fn reinhard_remap(scale: f64, nd: f64, invert: bool) -> f64 {
    if invert {
        if nd >= 1.0 {
            scale
        } else {
            scale * -(nd / (nd - 1.0))
        }
    } else {
        scale * nd / (1.0 + nd)
    }
}

fn toe(x: f64, limit: f64, k1_in: f64, k2_in: f64, invert: bool) -> f64 {
    if x > limit {
        return x;
    }
    let k2 = ctl_max(k2_in, 0.001);
    let k1 = (k1_in * k1_in + k2 * k2).sqrt();
    let k3 = (limit + k1) / (limit + k2);
    if invert {
        (x * x + k1 * x) / (k3 * (x + k2))
    } else {
        let minus_b = k3 * x - k1;
        let minus_c = k2 * k3 * x;
        0.5 * (minus_b + (minus_b * minus_b + 4.0 * minus_c).sqrt())
    }
}

fn chroma_compress_norm(h: f64, scale: f64) -> f64 {
    let hr = h / 180.0 * PI;
    let a = hr.cos();
    let b = hr.sin();
    let cos_hr2 = a * a - b * b;
    let sin_hr2 = 2.0 * a * b;
    let cos_hr3 = 4.0 * a * a * a - 3.0 * a;
    let sin_hr3 = 3.0 * b - 4.0 * b * b * b;
    let m = 11.34072 * a
        + 16.46899 * cos_hr2
        + 7.88380 * cos_hr3
        + 14.66441 * b
        + -6.37224 * sin_hr2
        + 9.19364 * sin_hr3
        + 77.12896;
    m * scale
}

fn chroma_compress_fwd(jmh: V3, tonemapped_j: f64, p: &OdtParams) -> V3 {
    let (j, m, h) = (jmh[0], jmh[1], jmh[2]);
    let mut m_compr = m;
    if m != 0.0 {
        let nj = tonemapped_j / p.limit_j_max;
        let snj = ctl_max(0.0, 1.0 - nj);
        let mnorm = chroma_compress_norm(h, p.chroma_compress_scale);
        let limit = nj.powf(p.model_gamma_inv) * reach_m_from_table(h, &p.table_reach_m) / mnorm;
        let toe_limit = limit - 0.001;
        let toe_snj_sat = snj * p.sat;
        let toe_sqrt_nj_sat_thr = (nj * nj + p.sat_thr).sqrt();
        let toe_nj_compr = nj * p.compr;
        m_compr = m * (tonemapped_j / j).powf(p.model_gamma_inv);
        m_compr /= mnorm;
        m_compr = limit
            - toe(
                limit - m_compr,
                toe_limit,
                toe_snj_sat,
                toe_sqrt_nj_sat_thr,
                false,
            );
        m_compr = toe(m_compr, limit, toe_nj_compr, snj, false);
        m_compr *= mnorm;
    }
    [tonemapped_j, m_compr, h]
}

fn chroma_compress_inv(jmh: V3, j: f64, p: &OdtParams) -> V3 {
    let (tonemapped_j, m_compr, h) = (jmh[0], jmh[1], jmh[2]);
    let mut m = m_compr;
    if m_compr != 0.0 {
        let nj = tonemapped_j / p.limit_j_max;
        let snj = ctl_max(0.0, 1.0 - nj);
        let mnorm = chroma_compress_norm(h, p.chroma_compress_scale);
        let limit = nj.powf(p.model_gamma_inv) * reach_m_from_table(h, &p.table_reach_m) / mnorm;
        let toe_limit = limit - 0.001;
        let toe_snj_sat = snj * p.sat;
        let toe_sqrt_nj_sat_thr = (nj * nj + p.sat_thr).sqrt();
        let toe_nj_compr = nj * p.compr;
        m = m_compr / mnorm;
        m = toe(m, limit, toe_nj_compr, snj, true);
        m = limit - toe(limit - m, toe_limit, toe_snj_sat, toe_sqrt_nj_sat_thr, true);
        m *= mnorm;
        m *= (tonemapped_j / j).powf(-p.model_gamma_inv);
    }
    [j, m, h]
}

fn tonemap_and_compress_fwd(jmh: V3, p: &OdtParams) -> V3 {
    let linear = j_to_y(jmh[0], &p.input_params) / REF_LUMINANCE;
    let tonemapped_y = tonescale_fwd(linear, &p.ts);
    let j_ts = y_to_j(tonemapped_y, &p.input_params);
    chroma_compress_fwd(jmh, j_ts, p)
}

fn tonemap_and_compress_inv(jmh_tc: V3, p: &OdtParams) -> V3 {
    let luminance = j_to_y(jmh_tc[0], &p.input_params);
    let linear = tonescale_inv(luminance / REF_LUMINANCE, &p.ts);
    let j = y_to_j(linear * REF_LUMINANCE, &p.input_params);
    chroma_compress_inv(jmh_tc, j, p)
}

fn compute_compression_vector_slope(
    intersect_j: f64,
    focus_j: f64,
    limit_j_max: f64,
    slope_gain: f64,
) -> f64 {
    let direction_scalar = if intersect_j < focus_j {
        intersect_j
    } else {
        limit_j_max - intersect_j
    };
    direction_scalar * (intersect_j - focus_j) / (focus_j * slope_gain)
}

fn solve_j_intersect(j: f64, m: f64, focus_j: f64, max_j: f64, slope_gain: f64) -> f64 {
    let m_scaled = m / slope_gain;
    let a = m_scaled / focus_j;
    if j < focus_j {
        let b = 1.0 - m_scaled;
        let c = -j;
        let det = b * b - 4.0 * a * c;
        -2.0 * c / (b + det.sqrt())
    } else {
        let b = -(1.0 + m_scaled + max_j * a);
        let c = max_j * m_scaled + j;
        let det = b * b - 4.0 * a * c;
        -2.0 * c / (b - det.sqrt())
    }
}

fn smin_scaled(a: f64, b: f64, scale_reference: f64) -> f64 {
    let s_scaled = SMOOTH_CUSPS * scale_reference;
    let h = ctl_max(s_scaled - (a - b).abs(), 0.0) / s_scaled;
    ctl_min(a, b) - h * h * h * s_scaled * (1.0 / 6.0)
}

fn estimate_line_and_boundary_intersection_m(
    j_axis_intersect: f64,
    slope: f64,
    inv_gamma: f64,
    j_max: f64,
    m_max: f64,
    j_intersection_reference: f64,
) -> f64 {
    let normalised_j = j_axis_intersect / j_intersection_reference;
    let shifted_intersection = j_intersection_reference * normalised_j.powf(inv_gamma);
    shifted_intersection * m_max / (j_max - slope * m_max)
}

fn find_gamut_boundary_intersection(
    jm_cusp: [f64; 2],
    j_max: f64,
    gamma_top_inv: f64,
    gamma_bottom_inv: f64,
    j_intersect_source: f64,
    slope: f64,
    j_intersect_cusp: f64,
) -> f64 {
    let lower = estimate_line_and_boundary_intersection_m(
        j_intersect_source,
        slope,
        gamma_bottom_inv,
        jm_cusp[0],
        jm_cusp[1],
        j_intersect_cusp,
    );
    let upper = estimate_line_and_boundary_intersection_m(
        j_max - j_intersect_source,
        -slope,
        gamma_top_inv,
        j_max - jm_cusp[0],
        jm_cusp[1],
        j_max - j_intersect_cusp,
    );
    smin_scaled(lower, upper, jm_cusp[1])
}

fn get_focus_gain(j: f64, analytical_threshold: f64, limit_j_max: f64, focus_dist: f64) -> f64 {
    let mut gain = limit_j_max * focus_dist;
    if j > analytical_threshold {
        let mut adj =
            ((limit_j_max - analytical_threshold) / ctl_max(0.0001, limit_j_max - j)).log10();
        adj = adj * adj + 1.0;
        gain *= adj;
    }
    gain
}

fn remap_m(m: f64, gamut_boundary_m: f64, reach_boundary_m: f64, invert: bool) -> f64 {
    let boundary_ratio = gamut_boundary_m / reach_boundary_m;
    let proportion = ctl_max(boundary_ratio, COMPRESSION_THRESHOLD);
    let threshold = proportion * gamut_boundary_m;
    if m <= threshold || proportion >= 1.0 {
        return m;
    }
    let m_offset = m - threshold;
    let gamut_offset = gamut_boundary_m - threshold;
    let reach_offset = reach_boundary_m - threshold;
    let scale = reach_offset / ((reach_offset / gamut_offset) - 1.0);
    let nd = m_offset / scale;
    threshold + reinhard_remap(scale, nd, invert)
}

#[derive(Clone, Copy, Debug)]
struct HueDependentGamutParams {
    jm_cusp: [f64; 2],
    gamma_bottom_inv: f64,
    gamma_top_inv: f64,
    focus_j: f64,
    analytical_threshold: f64,
}

fn compress_gamut(
    jmh: V3,
    jx: f64,
    p: &OdtParams,
    hdp: &HueDependentGamutParams,
    invert: bool,
) -> V3 {
    let (j, m, h) = (jmh[0], jmh[1], jmh[2]);
    let slope_gain = get_focus_gain(jx, hdp.analytical_threshold, p.limit_j_max, p.focus_dist);
    let j_intersect_source = solve_j_intersect(j, m, hdp.focus_j, p.limit_j_max, slope_gain);
    let gamut_slope = compute_compression_vector_slope(
        j_intersect_source,
        hdp.focus_j,
        p.limit_j_max,
        slope_gain,
    );
    let j_intersect_cusp = solve_j_intersect(
        hdp.jm_cusp[0],
        hdp.jm_cusp[1],
        hdp.focus_j,
        p.limit_j_max,
        slope_gain,
    );
    let gamut_boundary_m = find_gamut_boundary_intersection(
        hdp.jm_cusp,
        p.limit_j_max,
        hdp.gamma_top_inv,
        hdp.gamma_bottom_inv,
        j_intersect_source,
        gamut_slope,
        j_intersect_cusp,
    );
    if gamut_boundary_m <= 0.0 {
        return [j, 0.0, h];
    }
    let reach_max_m = reach_m_from_table(h, &p.table_reach_m);
    let reach_boundary_m = estimate_line_and_boundary_intersection_m(
        j_intersect_source,
        gamut_slope,
        p.model_gamma_inv,
        p.limit_j_max,
        reach_max_m,
        p.limit_j_max,
    );
    let remapped_m = remap_m(m, gamut_boundary_m, reach_boundary_m, invert);
    [j_intersect_source + remapped_m * gamut_slope, remapped_m, h]
}

fn cusp_from_table(h: f64, table: &[V3]) -> [f64; 2] {
    let mut low_i: i64 = 0;
    let mut high_i: i64 = (BASE_INDEX + TABLE_SIZE) as i64;
    let mut i = hue_position_in_uniform_table(h, TABLE_SIZE) + BASE_INDEX as i64;
    while low_i + 1 < high_i {
        if h > table[i as usize][2] {
            low_i = i;
        } else {
            high_i = i;
        }
        // CTL: `i = midpoint(low_i, high_i)` — a float midpoint truncated to int.
        i = ((low_i + high_i) as f64 / 2.0) as i64;
    }
    let lo = table[(high_i - 1) as usize];
    let hi = table[high_i as usize];
    let t = (h - lo[2]) / (hi[2] - lo[2]);
    [lerp(lo[0], hi[0], t), lerp(lo[1], hi[1], t)]
}

fn lookup_hue_interval(h: f64, hue_table: &[f64], range: [i64; 2]) -> usize {
    let mut i = BASE_INDEX as i64 + hue_position_in_uniform_table(h, TOTAL_TABLE_SIZE);
    let mut i_lo = (BASE_INDEX as i64).max(i + range[0]);
    let mut i_hi = ((BASE_INDEX + TABLE_SIZE) as i64).min(i + range[1]);
    while i_lo + 1 < i_hi {
        if h > hue_table[i as usize] {
            i_lo = i;
        } else {
            i_hi = i;
        }
        i = ((i_lo + i_hi) as f64 / 2.0) as i64;
    }
    i_hi.max(1) as usize
}

fn compute_focus_j(cusp_j: f64, mid_j: f64, limit_j_max: f64) -> f64 {
    lerp(
        cusp_j,
        mid_j,
        ctl_min(1.0, CUSP_MID_BLEND - (cusp_j / limit_j_max)),
    )
}

fn init_hue_dependent_gamut_params(hue: f64, p: &OdtParams) -> HueDependentGamutParams {
    let i_hi = lookup_hue_interval(hue, &p.table_hues, p.hue_linearity_search_range);
    let t = hue - p.table_hues[i_hi - 1];
    let jm_cusp = cusp_from_table(hue, &p.table_gamut_cusps);
    HueDependentGamutParams {
        jm_cusp,
        gamma_bottom_inv: p.lower_hull_gamma_inv,
        gamma_top_inv: lerp(
            p.table_upper_hull_gamma[i_hi - 1],
            p.table_upper_hull_gamma[i_hi],
            t,
        ),
        focus_j: compute_focus_j(jm_cusp[0], p.mid_j, p.limit_j_max),
        analytical_threshold: lerp(jm_cusp[0], p.limit_j_max, FOCUS_GAIN_BLEND),
    }
}

fn gamut_compress_fwd(jmh: V3, p: &OdtParams) -> V3 {
    let (j, m, h) = (jmh[0], jmh[1], jmh[2]);
    if j <= 0.0 {
        return [0.0, 0.0, h];
    }
    if m < 0.0 || j > p.limit_j_max {
        return [j, 0.0, h];
    }
    let hdp = init_hue_dependent_gamut_params(h, p);
    compress_gamut(jmh, j, p, &hdp, false)
}

fn gamut_compress_inv(jmh: V3, p: &OdtParams) -> V3 {
    let (j, m, h) = (jmh[0], jmh[1], jmh[2]);
    if j <= 0.0 {
        return [0.0, 0.0, h];
    }
    if m < 0.0 || j > p.limit_j_max {
        return [j, 0.0, h];
    }
    let hdp = init_hue_dependent_gamut_params(h, p);
    let mut jx = j;
    if jx > hdp.analytical_threshold {
        jx = compress_gamut(jmh, jx, p, &hdp, true)[0];
    }
    compress_gamut(jmh, jx, p, &hdp, true)
}

// ---- table building ---------------------------------------------------------

fn generate_unit_cube_cusp_corners(corner: usize) -> V3 {
    [
        if (corner + 1) % CUSP_CORNER_COUNT < 3 {
            1.0
        } else {
            0.0
        },
        if (corner + 5) % CUSP_CORNER_COUNT < 3 {
            1.0
        } else {
            0.0
        },
        if (corner + 3) % CUSP_CORNER_COUNT < 3 {
            1.0
        } else {
            0.0
        },
    ]
}

fn build_limiting_cusp_corners_tables(
    params: &JMhParams,
    peak_luminance: f64,
) -> (Vec<V3>, Vec<V3>) {
    let mut temp_rgb = [[0.0; 3]; CUSP_CORNER_COUNT];
    let mut temp_jmh = [[0.0; 3]; CUSP_CORNER_COUNT];
    let mut min_index = 0;
    for i in 0..CUSP_CORNER_COUNT {
        temp_rgb[i] =
            generate_unit_cube_cusp_corners(i).map(|v| v * peak_luminance / REF_LUMINANCE);
        temp_jmh[i] = rgb_to_jmh(temp_rgb[i], params);
        if temp_jmh[i][2] < temp_jmh[min_index][2] {
            min_index = i;
        }
    }
    let mut rgb = vec![[0.0; 3]; TOTAL_CORNER_COUNT];
    let mut jmh = vec![[0.0; 3]; TOTAL_CORNER_COUNT];
    for i in 0..CUSP_CORNER_COUNT {
        rgb[i + 1] = temp_rgb[(i + min_index) % CUSP_CORNER_COUNT];
        jmh[i + 1] = temp_jmh[(i + min_index) % CUSP_CORNER_COUNT];
    }
    rgb[0] = rgb[CUSP_CORNER_COUNT];
    rgb[CUSP_CORNER_COUNT + 1] = rgb[1];
    jmh[0] = jmh[CUSP_CORNER_COUNT];
    jmh[CUSP_CORNER_COUNT + 1] = jmh[1];
    jmh[0][2] -= HUE_LIMIT;
    jmh[CUSP_CORNER_COUNT + 1][2] += HUE_LIMIT;
    (rgb, jmh)
}

fn find_reach_corners_table(
    params_reach: &JMhParams,
    limit_j_max: f64,
    forward_limit: f64,
) -> Vec<V3> {
    let mut temp = [[0.0; 3]; CUSP_CORNER_COUNT];
    let limit_a = j_to_achromatic_n(limit_j_max, params_reach.inv_cz);
    let mut min_index = 0;
    for i in 0..CUSP_CORNER_COUNT {
        let rgb_vector = generate_unit_cube_cusp_corners(i);
        let mut lower = 0.0;
        let mut upper = forward_limit;
        while (upper - lower) > REACH_CUSP_TOLERANCE {
            let test = (lower + upper) / 2.0;
            let a = rgb_to_aab(rgb_vector.map(|v| v * test), params_reach)[0];
            if a < limit_a {
                lower = test;
            } else {
                upper = test;
            }
        }
        temp[i] = rgb_to_jmh(rgb_vector.map(|v| v * upper), params_reach);
        if temp[i][2] < temp[min_index][2] {
            min_index = i;
        }
    }
    let mut out = vec![[0.0; 3]; TOTAL_CORNER_COUNT];
    for i in 0..CUSP_CORNER_COUNT {
        out[i + 1] = temp[(i + min_index) % CUSP_CORNER_COUNT];
    }
    out[0] = out[CUSP_CORNER_COUNT];
    out[CUSP_CORNER_COUNT + 1] = out[1];
    out[0][2] -= HUE_LIMIT;
    out[CUSP_CORNER_COUNT + 1][2] += HUE_LIMIT;
    out
}

fn extract_sorted_cube_hues(reach: &[V3], limit: &[V3]) -> [f64; MAX_SORTED_CORNERS] {
    let mut sorted = [0.0; MAX_SORTED_CORNERS];
    let (mut idx, mut ri, mut li) = (0usize, 1usize, 1usize);
    while ri < CUSP_CORNER_COUNT + 1 || li < CUSP_CORNER_COUNT + 1 {
        let rh = reach[ri][2];
        let lh = limit[li][2];
        if rh == lh {
            sorted[idx] = rh;
            ri += 1;
            li += 1;
        } else if rh < lh {
            sorted[idx] = rh;
            ri += 1;
        } else {
            sorted[idx] = lh;
            li += 1;
        }
        idx += 1;
        // Deviation from a literal transcription: CTL arrays are fixed-size and
        // silently index past the end when one list is exhausted; the result is
        // only ever read for the first MAX_SORTED_CORNERS entries.
        if idx == MAX_SORTED_CORNERS {
            break;
        }
    }
    sorted
}

fn build_hue_sample_interval(samples: i64, lower: f64, upper: f64, table: &mut [f64], base: i64) {
    let delta = (upper - lower) / samples as f64;
    for i in 0..samples.max(0) {
        let at = (base + i) as usize;
        if at < table.len() {
            table[at] = lower + i as f64 * delta;
        }
    }
}

fn build_hue_table(sorted_hues: &[f64; MAX_SORTED_CORNERS]) -> Vec<f64> {
    let mut hue_table = vec![0.0; TOTAL_TABLE_SIZE];
    let ideal_spacing = TABLE_SIZE as f64 / HUE_LIMIT;
    let mut samples_count = [0i64; 2 * CUSP_CORNER_COUNT + 2];
    let mut last_idx: i64 = 0;
    let mut min_index: i64 = if sorted_hues[0] == 0.0 { 0 } else { 1 };
    for hue_idx in 0..MAX_SORTED_CORNERS {
        let mut nominal_idx = ctl_round(sorted_hues[hue_idx] * ideal_spacing)
            .max(min_index)
            .min(TABLE_SIZE as i64 - 1);
        if last_idx == nominal_idx {
            if hue_idx > 1 && samples_count[hue_idx - 2] != samples_count[hue_idx - 1] - 1 {
                samples_count[hue_idx - 1] -= 1;
            } else {
                nominal_idx += 1;
            }
        }
        samples_count[hue_idx] = nominal_idx.min(TABLE_SIZE as i64 - 1);
        min_index = nominal_idx;
        last_idx = min_index;
    }
    let mut total_samples: i64 = 0;
    let mut i = 0;
    build_hue_sample_interval(
        samples_count[i],
        0.0,
        sorted_hues[i],
        &mut hue_table,
        total_samples + 1,
    );
    total_samples += samples_count[i];
    i += 1;
    while i != MAX_SORTED_CORNERS {
        let samples = samples_count[i] - samples_count[i - 1];
        build_hue_sample_interval(
            samples,
            sorted_hues[i - 1],
            sorted_hues[i],
            &mut hue_table,
            total_samples + 1,
        );
        total_samples += samples;
        i += 1;
    }
    build_hue_sample_interval(
        TABLE_SIZE as i64 - total_samples,
        sorted_hues[i - 1],
        HUE_LIMIT,
        &mut hue_table,
        total_samples + 1,
    );
    hue_table[0] = hue_table[BASE_INDEX + TABLE_SIZE - 1] - HUE_LIMIT;
    hue_table[BASE_INDEX + TABLE_SIZE] = hue_table[BASE_INDEX] + HUE_LIMIT;
    hue_table
}

fn find_display_cusp_for_hue(
    hue: f64,
    rgb_corners: &[V3],
    jmh_corners: &[V3],
    params: &JMhParams,
) -> [f64; 2] {
    let mut upper_corner = 1;
    for (i, c) in jmh_corners.iter().enumerate().skip(1) {
        if c[2] > hue {
            upper_corner = i;
            break;
        }
    }
    let lower_corner = upper_corner - 1;
    if jmh_corners[lower_corner][2] == hue {
        return [jmh_corners[lower_corner][0], jmh_corners[lower_corner][1]];
    }
    let cusp_lower = rgb_corners[lower_corner];
    let cusp_upper = rgb_corners[upper_corner];
    let mut lower_t = 0.0;
    let mut upper_t = 1.0;
    while (upper_t - lower_t) > DISPLAY_CUSP_TOLERANCE {
        let sample_t = (lower_t + upper_t) / 2.0;
        let jmh = rgb_to_jmh(lerp_v(cusp_lower, cusp_upper, sample_t), params);
        if jmh[2] < jmh_corners[lower_corner][2] {
            upper_t = sample_t;
        } else if jmh[2] >= jmh_corners[upper_corner][2] {
            lower_t = sample_t;
        } else if jmh[2] > hue {
            upper_t = sample_t;
        } else {
            lower_t = sample_t;
        }
    }
    let sample_t = (lower_t + upper_t) / 2.0;
    let jmh = rgb_to_jmh(lerp_v(cusp_lower, cusp_upper, sample_t), params);
    [jmh[0], jmh[1]]
}

fn build_cusp_table(
    hue_table: &[f64],
    rgb_corners: &[V3],
    jmh_corners: &[V3],
    params: &JMhParams,
) -> Vec<V3> {
    let mut out = vec![[0.0; 3]; TOTAL_TABLE_SIZE];
    for i in BASE_INDEX..TOTAL_TABLE_SIZE {
        let hue = hue_table[i];
        let jm = find_display_cusp_for_hue(hue, rgb_corners, jmh_corners, params);
        out[i] = [jm[0], jm[1] * (1.0 + SMOOTH_M * SMOOTH_CUSPS), hue];
    }
    out[0] = [out[TABLE_SIZE][0], out[TABLE_SIZE][1], hue_table[0]];
    out[BASE_INDEX + TABLE_SIZE] = [
        out[BASE_INDEX][0],
        out[BASE_INDEX][1],
        hue_table[BASE_INDEX + TABLE_SIZE],
    ];
    out
}

fn make_reach_m_table(params: &JMhParams, limit_j_max: f64) -> Vec<f64> {
    let mut table = vec![0.0; TOTAL_TABLE_SIZE];
    for i in 0..TABLE_SIZE {
        let hue = base_hue_for_position(i as i64, TABLE_SIZE);
        let search_range = 50.0;
        let search_maximum = 1300.0;
        let mut low = 0.0;
        let mut high = low + search_range;
        let mut outside = false;
        while !outside && high < search_maximum {
            let rgb = jmh_to_rgb([limit_j_max, high, hue], params);
            outside = rgb.iter().any(|v| *v < 0.0);
            if !outside {
                low = high;
                high += search_range;
            }
        }
        while high - low > 1e-2 {
            let sample_m = (high + low) / 2.0;
            let rgb = jmh_to_rgb([limit_j_max, sample_m, hue], params);
            if rgb.iter().any(|v| *v < 0.0) {
                high = sample_m;
            } else {
                low = sample_m;
            }
        }
        table[i + BASE_INDEX] = high;
    }
    table[0] = table[TABLE_SIZE];
    table[BASE_INDEX + TABLE_SIZE] = table[BASE_INDEX];
    table
}

const TEST_POSITIONS: [f64; 5] = [0.01, 0.1, 0.5, 0.8, 0.99];

fn make_upper_hull_gamma_table(cusps: &[V3], p: &OdtParams) -> Vec<f64> {
    let mut out = vec![0.0; TOTAL_TABLE_SIZE];
    for i in BASE_INDEX..BASE_INDEX + TABLE_SIZE {
        let hue = cusps[i][2];
        let jm_cusp = [cusps[i][0], cusps[i][1]];
        let analytical_threshold = lerp(jm_cusp[0], p.limit_j_max, FOCUS_GAIN_BLEND);
        let focus_j = compute_focus_j(jm_cusp[0], p.mid_j, p.limit_j_max);
        let tests: Vec<(V3, f64, f64, f64)> = TEST_POSITIONS
            .iter()
            .map(|pos| {
                let test_j = lerp(jm_cusp[0], p.limit_j_max, *pos);
                let slope_gain =
                    get_focus_gain(test_j, analytical_threshold, p.limit_j_max, p.focus_dist);
                let j_intersect =
                    solve_j_intersect(test_j, jm_cusp[1], focus_j, p.limit_j_max, slope_gain);
                let slope = compute_compression_vector_slope(
                    j_intersect,
                    focus_j,
                    p.limit_j_max,
                    slope_gain,
                );
                let j_cusp =
                    solve_j_intersect(jm_cusp[0], jm_cusp[1], focus_j, p.limit_j_max, slope_gain);
                ([test_j, jm_cusp[1], hue], j_intersect, slope, j_cusp)
            })
            .collect();
        let fits = |top_gamma_inv: f64| {
            let luminance_limit = p.peak_luminance / REF_LUMINANCE;
            tests.iter().all(|(jmh, j_intersect, slope, j_cusp)| {
                let m = find_gamut_boundary_intersection(
                    jm_cusp,
                    p.limit_j_max,
                    top_gamma_inv,
                    p.lower_hull_gamma_inv,
                    *j_intersect,
                    *slope,
                    *j_cusp,
                );
                let j = j_intersect + slope * m;
                let rgb = jmh_to_rgb([j, m, jmh[2]], &p.limit_params);
                rgb.iter().any(|v| *v > luminance_limit)
            })
        };
        let search_range = GAMMA_SEARCH_STEP;
        let mut low = GAMMA_MINIMUM;
        let mut high = low + search_range;
        let mut outside = false;
        while !outside && high < GAMMA_MAXIMUM {
            if fits(1.0 / high) {
                outside = true;
            } else {
                low = high;
                high += search_range;
            }
        }
        while (high - low) > GAMMA_ACCURACY {
            let test_gamma = (high + low) / 2.0;
            if fits(1.0 / test_gamma) {
                high = test_gamma;
            } else {
                low = test_gamma;
            }
        }
        out[i] = 1.0 / high;
    }
    out[0] = out[TABLE_SIZE];
    out[TABLE_SIZE + BASE_INDEX] = out[BASE_INDEX];
    out
}

fn determine_hue_linearity_search_range(hue_table: &[f64]) -> [i64; 2] {
    let mut range = [0i64, 1i64];
    for (i, h) in hue_table
        .iter()
        .enumerate()
        .take(BASE_INDEX + TABLE_SIZE)
        .skip(BASE_INDEX)
    {
        let pos = hue_position_in_uniform_table(*h, TOTAL_TABLE_SIZE);
        let delta = i as i64 - pos;
        range[0] = range[0].min(delta);
        range[1] = range[1].max(delta + 1);
    }
    range
}

impl OdtParams {
    /// CTL `init_ODTParams`. Building the tables costs tens of milliseconds;
    /// build once per display and reuse.
    pub fn new(peak_luminance: f64, limiting_primaries: &Chromaticities) -> OdtParams {
        let input_params = init_jmh_params(&AP0);
        let reach_params = init_jmh_params(&AP1);
        let limit_params = init_jmh_params(limiting_primaries);
        let ts = init_ts_params(peak_luminance);
        let limit_j_max = y_to_j(peak_luminance, &input_params);
        let mut p = OdtParams {
            peak_luminance,
            input_params,
            reach_params,
            limit_params,
            ts,
            limit_j_max,
            model_gamma_inv: 1.0 / model_gamma(),
            table_reach_m: make_reach_m_table(&reach_params, limit_j_max),
            sat: ctl_max(
                0.2,
                CHROMA_EXPAND - (CHROMA_EXPAND * CHROMA_EXPAND_FACT) * ts.log_peak,
            ),
            sat_thr: CHROMA_EXPAND_THR / peak_luminance,
            compr: CHROMA_COMPRESS + (CHROMA_COMPRESS * CHROMA_COMPRESS_FACT) * ts.log_peak,
            chroma_compress_scale: (0.03379 * peak_luminance).powf(0.30596) - 0.45135,
            mid_j: y_to_j(ts.c_t * REF_LUMINANCE, &input_params),
            focus_dist: FOCUS_DISTANCE + FOCUS_DISTANCE * FOCUS_DISTANCE_SCALING * ts.log_peak,
            lower_hull_gamma_inv: 1.0 / (1.14 + 0.07 * ts.log_peak),
            table_hues: Vec::new(),
            table_gamut_cusps: Vec::new(),
            table_upper_hull_gamma: Vec::new(),
            hue_linearity_search_range: [0, 1],
            limit_rgb_to_xyz: rgb_to_xyz(limiting_primaries, 1.0),
            ap0_to_ap1: mul_m_m(&rgb_to_xyz(&AP0, 1.0), &xyz_to_rgb(&AP1, 1.0)),
            ap1_to_ap0: mul_m_m(&rgb_to_xyz(&AP1, 1.0), &xyz_to_rgb(&AP0, 1.0)),
        };
        let reach_corners =
            find_reach_corners_table(&p.reach_params, p.limit_j_max, p.ts.forward_limit);
        let (limit_rgb, limit_jmh) =
            build_limiting_cusp_corners_tables(&p.limit_params, peak_luminance);
        let sorted = extract_sorted_cube_hues(&reach_corners, &limit_jmh);
        let hue_table = build_hue_table(&sorted);
        p.table_gamut_cusps = build_cusp_table(&hue_table, &limit_rgb, &limit_jmh, &p.limit_params);
        p.table_hues = p.table_gamut_cusps.iter().map(|c| c[2]).collect();
        p.table_upper_hull_gamma = make_upper_hull_gamma_table(&p.table_gamut_cusps.clone(), &p);
        p.hue_linearity_search_range = determine_hue_linearity_search_range(&p.table_hues);
        p
    }

    /// CTL `outputTransform_fwd`: ACES2065-1 in, limiting-primaries RGB out
    /// (display linear, 1.0 = 100 nits).
    pub fn forward(&self, aces: V3) -> V3 {
        let (ap0_to_ap1, ap1_to_ap0) = (&self.ap0_to_ap1, &self.ap1_to_ap0);
        let ap1 =
            mul_v_m(aces, ap0_to_ap1).map(|v| ctl_max(0.0, ctl_min(v, self.ts.forward_limit)));
        let ap0_clamped = mul_v_m(ap1, ap1_to_ap0);
        let jmh = rgb_to_jmh(ap0_clamped, &self.input_params);
        let tonemapped = tonemap_and_compress_fwd(jmh, self);
        let compressed = gamut_compress_fwd(tonemapped, self);
        jmh_to_rgb(compressed, &self.limit_params)
    }

    /// CTL `outputTransform_inv`.
    pub fn inverse(&self, rgb_out: V3) -> V3 {
        let compressed = rgb_to_jmh(rgb_out, &self.limit_params);
        let tonemapped = gamut_compress_inv(compressed, self);
        let jmh = tonemap_and_compress_inv(tonemapped, self);
        jmh_to_rgb(jmh, &self.input_params)
    }
}

/// The OpenColorIO style prefix of every ACES 2.0 output transform.
pub const STYLE_PREFIX: &str = "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - ";

/// Every ACES 2.0 output transform OpenColorIO 2.5 registers, by the part of
/// the style name between [`STYLE_PREFIX`] and `_2.0`. Each is checked against
/// OpenColorIO in both directions (`tests/aces2_oracle.rs`); a name not on this
/// list is not answered, however plausible its parse.
pub const DISPLAYS: [&str; 31] = [
    "SDR-100nit-REC709",
    "SDR-100nit-P3-D65",
    "HDR-108nit-P3-D65",
    "HDR-300nit-P3-D65",
    "HDR-500nit-P3-D65",
    "HDR-1000nit-P3-D65",
    "HDR-2000nit-P3-D65",
    "HDR-4000nit-P3-D65",
    "HDR-500nit-REC2020",
    "HDR-1000nit-REC2020",
    "HDR-2000nit-REC2020",
    "HDR-4000nit-REC2020",
    "SDR-100nit-REC709-D60-in-REC709-D65",
    "SDR-100nit-REC709-D60-in-P3-D65",
    "SDR-100nit-REC709-D60-in-REC2020-D65",
    "SDR-100nit-P3-D60-in-P3-D65",
    "SDR-100nit-P3-D60-in-XYZ-E",
    "HDR-108nit-P3-D60-in-P3-D65",
    "HDR-300nit-P3-D60-in-XYZ-E",
    "HDR-500nit-P3-D60-in-P3-D65",
    "HDR-1000nit-P3-D60-in-P3-D65",
    "HDR-2000nit-P3-D60-in-P3-D65",
    "HDR-4000nit-P3-D60-in-P3-D65",
    "HDR-500nit-P3-D60-in-REC2020-D65",
    "HDR-1000nit-P3-D60-in-REC2020-D65",
    "HDR-2000nit-P3-D60-in-REC2020-D65",
    "HDR-4000nit-P3-D60-in-REC2020-D65",
    "HDR-500nit-REC2020-D60-in-REC2020-D65",
    "HDR-1000nit-REC2020-D60-in-REC2020-D65",
    "HDR-2000nit-REC2020-D60-in-REC2020-D65",
    "HDR-4000nit-REC2020-D60-in-REC2020-D65",
];

/// CIE XYZ as primaries, with the equal-energy white.
pub const XYZ_E: Chromaticities = Chromaticities {
    red: [1.0, 0.0],
    green: [0.0, 1.0],
    blue: [0.0, 0.0],
    white: [1.0 / 3.0, 1.0 / 3.0],
};

fn with_white(c: Chromaticities, white: [f64; 2]) -> Chromaticities {
    Chromaticities { white, ..c }
}

fn named_primaries(name: &str) -> Option<Chromaticities> {
    Some(match name {
        "REC709" | "REC709-D65" => REC709,
        "P3-D65" => P3_D65,
        "REC2020" | "REC2020-D65" => REC2020,
        "REC709-D60" => with_white(REC709, AP0.white),
        "P3-D60" => with_white(P3_D65, AP0.white),
        "REC2020-D60" => with_white(REC2020, AP0.white),
        "XYZ-E" => XYZ_E,
        _ => return None,
    })
}

/// One display's output transform, as OpenColorIO's builtin builds it:
/// ACES2065-1 in, CIE XYZ out (1.0 = 100 nits), relative to the limiting white
/// without chromatic adaptation. When the limiting white is not the encoding
/// white (the "D60 sim" displays), the result is scaled so the limiting white
/// fits inside the encoding primaries.
#[derive(Clone, Debug)]
pub struct DisplayTransform {
    pub display: String,
    pub params: OdtParams,
    /// Scales display light after clamping: the limiting white fitted into the
    /// encoding primaries, times the cinema displays' linear scale.
    white_scale: f64,
    xyz_to_limit_rgb: M3,
}

impl DisplayTransform {
    /// The transform for a name on [`DISPLAYS`]; `None` for any other.
    pub fn new(display: &str) -> Option<DisplayTransform> {
        if !DISPLAYS.contains(&display) {
            return None;
        }
        let (limit, encoding) = match display.split_once("-in-") {
            Some((l, e)) => (l, Some(e)),
            None => (display, None),
        };
        let mut parts = limit.splitn(3, '-');
        let _range = parts.next()?;
        let nominal: f64 = parts.next()?.strip_suffix("nit")?.parse().ok()?;
        let limit_prims = named_primaries(parts.next()?)?;
        // The cinema HDR displays are rendered for a brighter peak and scaled
        // down: 108 nits is the 225-nit rendering at 0.48, 300 nits the
        // 625-nit one. Measured from OpenColorIO's builtins, which build them
        // that way; the rendering at the nominal peak does not match.
        let rendered = match nominal as u32 {
            108 => 225.0,
            300 => 625.0,
            _ => nominal,
        };
        let params = OdtParams::new(rendered, &limit_prims);
        let fit_white = match encoding {
            None => 1.0,
            // The DCDM-style SDR encoding carries D60 white in XYZ unscaled;
            // OpenColorIO applies no white scale to it (measured), while its
            // 300-nit XYZ-E sibling does.
            Some(_) if display == "SDR-100nit-P3-D60-in-XYZ-E" => 1.0,
            Some(e) => {
                let limit_to_display = mul_m_m(
                    &rgb_to_xyz(&limit_prims, 1.0),
                    &xyz_to_rgb(&named_primaries(e)?, 1.0),
                );
                let w = mul_v_m([1.0; 3], &limit_to_display);
                1.0 / ctl_max(ctl_max(w[0], w[1]), w[2])
            }
        };
        let white_scale = fit_white * nominal / rendered;
        Some(DisplayTransform {
            display: display.to_string(),
            xyz_to_limit_rgb: invert(&params.limit_rgb_to_xyz),
            params,
            white_scale,
        })
    }

    /// The transform for a full OpenColorIO style name, e.g.
    /// `ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-100nit-REC709_2.0`.
    pub fn for_style(style: &str) -> Option<DisplayTransform> {
        DisplayTransform::new(style.strip_prefix(STYLE_PREFIX)?.strip_suffix("_2.0")?)
    }

    fn peak(&self) -> f64 {
        self.params.peak_luminance / REF_LUMINANCE
    }

    /// ACES2065-1 to CIE XYZ.
    pub fn to_xyz(&self, aces: V3) -> V3 {
        let peak = self.peak();
        let rgb = self
            .params
            .forward(aces)
            .map(|v| ctl_max(0.0, ctl_min(v, peak)) * self.white_scale);
        mul_v_m(rgb, &self.params.limit_rgb_to_xyz)
    }

    /// CIE XYZ to ACES2065-1: the display colour's scene origin. Display
    /// colour outside the limiting gamut or above peak is clamped first, as
    /// OpenColorIO does.
    pub fn from_xyz(&self, xyz: V3) -> V3 {
        let peak = self.peak();
        let rgb = mul_v_m(xyz, &self.xyz_to_limit_rgb)
            .map(|v| ctl_max(0.0, ctl_min(v / self.white_scale, peak)));
        let aces = self.params.inverse(rgb);
        let p = &self.params;
        let ap1 =
            mul_v_m(aces, &p.ap0_to_ap1).map(|v| ctl_max(0.0, ctl_min(v, p.ts.forward_limit)));
        mul_v_m(ap1, &p.ap1_to_ap0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_matrices_are_the_ones_opencolorio_builds() {
        // AP0 to AP1, as OpenColorIO prints it (column-vector rows).
        let m = mul_m_m(&rgb_to_xyz(&AP0, 1.0), &xyz_to_rgb(&AP1, 1.0));
        let expect = [
            [1.451439316145665, -0.2365107468937402, -0.2149285692519252],
            [-0.07655377339602043, 1.176229699833573, -0.0996759264375522],
            [
                0.008316148425697719,
                -0.006032449791021028,
                0.9977163013653233,
            ],
        ];
        for i in 0..3 {
            for j in 0..3 {
                assert!((m[j][i] - expect[i][j]).abs() < 1e-12, "[{i}][{j}]");
            }
        }
    }

    #[test]
    fn the_tonescale_puts_grey_where_the_standard_says() {
        let ts = init_ts_params(100.0);
        let grey = tonescale_fwd(0.18, &ts);
        // c_d = 10.013 is the fit's target; the fitted curve lands 18% grey at
        // 9.99993 nits, which is also what OpenColorIO 2.5.2 renders.
        assert!((grey - 9.99993).abs() < 1e-4, "{grey}");
        assert_eq!(ts.forward_limit, 1024.0);
        let x = tonescale_inv(tonescale_fwd(0.4, &ts) / 100.0 * 100.0 / 100.0, &ts);
        assert!((x - 0.4).abs() < 1e-9, "{x}");
    }

    #[test]
    fn the_inverse_undoes_the_forward_inside_the_gamut() {
        let p = OdtParams::new(100.0, &REC709);
        for aces in [[0.18, 0.18, 0.18], [0.3, 0.2, 0.1], [0.05, 0.1, 0.2]] {
            let back = p.inverse(p.forward(aces));
            for c in 0..3 {
                assert!((back[c] - aces[c]).abs() < 1e-3, "{aces:?} -> {back:?}");
            }
        }
    }
}
