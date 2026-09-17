//! Planar YUV to RGB: the matrix policy, and the CPU reference.
//!
//! ## Why the matrix is a decision, not a constant
//!
//! Converting YUV to RGB requires knowing which matrix encoded it. Real files
//! frequently do not say. The three major tools disagree about what to do then:
//! libswscale defaults to BT.601 unconditionally with no resolution heuristic,
//! mpv applies a two-way heuristic and never guesses BT.2020, and Apple treats
//! untagged media as SMPTE-C. **The same untagged 1080p file therefore decodes
//! three different ways in three different applications**, and none of them
//! tells the user it guessed.
//!
//! So this module refuses to guess silently. [`MatrixChoice`] carries not only
//! which matrix is used but *why*, and an assumption is a distinct state from a
//! declared value. The viewer is expected to show that state.

use serde::{Deserialize, Serialize};

/// The luma coefficients that define a YCbCr matrix.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Matrix {
    pub kr: f32,
    pub kb: f32,
}

impl Matrix {
    /// ITU-R BT.601 — standard definition.
    pub const BT601: Matrix = Matrix {
        kr: 0.299,
        kb: 0.114,
    };
    /// ITU-R BT.709 — high definition.
    pub const BT709: Matrix = Matrix {
        kr: 0.2126,
        kb: 0.0722,
    };
    /// ITU-R BT.2020 — ultra high definition, non-constant luminance.
    pub const BT2020_NCL: Matrix = Matrix {
        kr: 0.2627,
        kb: 0.0593,
    };

    /// The matrix for an ITU-T H.273 `matrix_coefficients` code point.
    ///
    /// Returns `None` for code points this build does not implement, including
    /// 0 (identity/RGB) and 3 (reserved), rather than substituting a neighbour.
    pub fn from_h273(code: u8) -> Option<Matrix> {
        match code {
            1 => Some(Matrix::BT709),
            // 5 (BT.470BG) and 6 (SMPTE 170M) share BT.601's coefficients.
            5 | 6 => Some(Matrix::BT601),
            9 => Some(Matrix::BT2020_NCL),
            _ => None,
        }
    }

    pub fn kg(&self) -> f32 {
        1.0 - self.kr - self.kb
    }
}

/// Which matrix is in use, and on what authority.
///
/// The distinction is the point: `Declared` came from the file, `Assumed` is
/// this build's choice in the absence of one. They must never be shown to a
/// user as the same thing, and a master must never be produced from an
/// `Assumed` matrix without someone confirming it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum MatrixChoice {
    /// The file declared an H.273 code point this build understands.
    Declared { code: u8, matrix: Matrix },
    /// The file declared nothing usable; this matrix was assumed.
    Assumed {
        matrix: Matrix,
        reason: &'static str,
    },
}

impl MatrixChoice {
    pub fn matrix(&self) -> Matrix {
        match self {
            MatrixChoice::Declared { matrix, .. } | MatrixChoice::Assumed { matrix, .. } => *matrix,
        }
    }

    pub fn is_assumed(&self) -> bool {
        matches!(self, MatrixChoice::Assumed { .. })
    }

    /// Human-readable, for the viewer's colour indicator.
    pub fn describe(&self) -> String {
        match self {
            MatrixChoice::Declared { code, matrix } => {
                format!("{} (declared, H.273 code {code})", name_of(*matrix))
            }
            MatrixChoice::Assumed { matrix, reason } => {
                format!("{} (ASSUMED — {reason})", name_of(*matrix))
            }
        }
    }

    /// Resolve the matrix for a frame of the given height.
    ///
    /// When the file declares a code point, that wins outright. When it does
    /// not, the fallback is by resolution — the convention SD content is
    /// BT.601 and HD and above is BT.709 — and the result is marked `Assumed`
    /// so nothing downstream can mistake it for knowledge. BT.2020 is never
    /// assumed: guessing a wide gamut is far more damaging than guessing a
    /// narrow one, because it stretches colour that was never that saturated.
    pub fn resolve(matrix_coefficients: Option<u8>, height: u32) -> MatrixChoice {
        if let Some(code) = matrix_coefficients {
            if let Some(matrix) = Matrix::from_h273(code) {
                return MatrixChoice::Declared { code, matrix };
            }
        }
        if height <= 576 {
            MatrixChoice::Assumed {
                matrix: Matrix::BT601,
                reason: "no matrix_coefficients in the file; standard-definition height",
            }
        } else {
            MatrixChoice::Assumed {
                matrix: Matrix::BT709,
                reason: "no matrix_coefficients in the file; high-definition height",
            }
        }
    }
}

fn name_of(m: Matrix) -> &'static str {
    if m == Matrix::BT709 {
        "BT.709"
    } else if m == Matrix::BT601 {
        "BT.601"
    } else if m == Matrix::BT2020_NCL {
        "BT.2020 NCL"
    } else {
        "custom matrix"
    }
}

/// How code values map onto the signal range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Range {
    /// Studio swing: 16..235 luma, 16..240 chroma at 8 bits.
    Limited,
    /// Full swing: 0..255.
    Full,
}

impl Range {
    /// `(y_offset, y_gain, c_offset, c_gain)` for planes normalised to 0..1.
    pub fn normalisation(&self) -> (f32, f32, f32, f32) {
        match self {
            Range::Limited => (16.0 / 255.0, 255.0 / 219.0, 128.0 / 255.0, 255.0 / 224.0),
            Range::Full => (0.0, 1.0, 128.0 / 255.0, 1.0),
        }
    }
}

/// Everything needed to convert one frame, resolved and explicit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct YuvConversion {
    pub choice: MatrixChoice,
    pub range: Range,
    pub width: u32,
    pub height: u32,
    pub chroma_width: u32,
    pub chroma_height: u32,
}

impl YuvConversion {
    /// The uniform block the shader expects, in declaration order.
    pub fn params(&self) -> [f32; 12] {
        let m = self.choice.matrix();
        let (yo, yg, co, cg) = self.range.normalisation();
        [
            f32::from_bits(self.width),
            f32::from_bits(self.height),
            f32::from_bits(self.chroma_width),
            f32::from_bits(self.chroma_height),
            m.kr,
            m.kb,
            yo,
            yg,
            co,
            cg,
            0.0,
            0.0,
        ]
    }
}

/// CPU reference conversion — the definition the GPU path is checked against.
///
/// Planes are normalised code values (byte / 255). Output is RGBA, alpha 1.
pub fn yuv_to_rgb_cpu(
    conv: &YuvConversion,
    plane_y: &[f32],
    plane_u: &[f32],
    plane_v: &[f32],
) -> Vec<f32> {
    let m = conv.choice.matrix();
    let (yo, yg, co, cg) = conv.range.normalisation();
    let (w, h) = (conv.width as usize, conv.height as usize);
    let (cw, ch) = (conv.chroma_width as usize, conv.chroma_height as usize);
    let kg = m.kg();

    let mut out = vec![0.0f32; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let cx = (x * cw / w).min(cw.saturating_sub(1));
            let cy = (y * ch / h).min(ch.saturating_sub(1));
            let yi = y * w + x;
            let ci = cy * cw + cx;

            let luma = (plane_y[yi] - yo) * yg;
            let cb = (plane_u[ci] - co) * cg;
            let cr = (plane_v[ci] - co) * cg;

            let r = luma + 2.0 * (1.0 - m.kr) * cr;
            let b = luma + 2.0 * (1.0 - m.kb) * cb;
            let g = (luma - m.kr * r - m.kb * b) / kg;

            out[yi * 4] = r;
            out[yi * 4 + 1] = g;
            out[yi * 4 + 2] = b;
            out[yi * 4 + 3] = 1.0;
        }
    }
    out
}

/// The inverse of [`yuv_to_rgb_cpu`], for encoding: RGBA code values to 8-bit
/// 4:2:0 planes under `conv`'s matrix and range.
///
/// Chroma is the mean of each 2×2 block. That is the least-squares choice for
/// the reference conversion, which reads one chroma sample for its whole block,
/// so a picture converted here and back through [`yuv_to_rgb_cpu`] is as close
/// to the original as 4:2:0 at 8 bits allows. RGB outside the range the
/// encoding can carry is clipped at the code-value limits; the count of clipped
/// samples is returned so a caller can report it.
pub fn rgb_to_yuv8_cpu(conv: &YuvConversion, rgba: &[f32]) -> ([Vec<u8>; 3], u64) {
    let m = conv.choice.matrix();
    let (w, h) = (conv.width as usize, conv.height as usize);
    let (cw, ch) = (conv.chroma_width as usize, conv.chroma_height as usize);
    let kg = m.kg();
    let (ylo, yspan, cmid, cspan) = match conv.range {
        Range::Limited => (16.0f64, 219.0f64, 128.0f64, 224.0f64),
        Range::Full => (0.0, 255.0, 128.0, 255.0),
    };
    let mut clipped = 0u64;
    let mut code = |v: f64| -> u8 {
        let r = v.round();
        if !(0.0..=255.0).contains(&r) {
            clipped += 1;
        }
        r.clamp(0.0, 255.0) as u8
    };
    let mut plane_y = vec![0u8; w * h];
    let mut sum_cb = vec![0.0f64; cw * ch];
    let mut sum_cr = vec![0.0f64; cw * ch];
    let mut count = vec![0u32; cw * ch];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let (r, g, b) = (
                f64::from(rgba[i]),
                f64::from(rgba[i + 1]),
                f64::from(rgba[i + 2]),
            );
            let luma = f64::from(m.kr) * r + f64::from(kg) * g + f64::from(m.kb) * b;
            plane_y[y * w + x] = code(ylo + yspan * luma);
            let cb = (b - luma) / (2.0 * (1.0 - f64::from(m.kb)));
            let cr = (r - luma) / (2.0 * (1.0 - f64::from(m.kr)));
            let cx = (x * cw / w).min(cw.saturating_sub(1));
            let cy = (y * ch / h).min(ch.saturating_sub(1));
            let ci = cy * cw + cx;
            sum_cb[ci] += cb;
            sum_cr[ci] += cr;
            count[ci] += 1;
        }
    }
    let mut plane_u = vec![0u8; cw * ch];
    let mut plane_v = vec![0u8; cw * ch];
    for ci in 0..cw * ch {
        let n = f64::from(count[ci].max(1));
        plane_u[ci] = code(cmid + cspan * sum_cb[ci] / n);
        plane_v[ci] = code(cmid + cspan * sum_cr[ci] / n);
    }
    ([plane_y, plane_u, plane_v], clipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encoding and decoding again lands every sample within one code value of
    /// where it started, for pictures whose chroma is constant per 2×2 block —
    /// the pictures 4:2:0 can carry exactly.
    #[test]
    fn rgb_to_yuv_inverts_the_reference_conversion() {
        for m in [Matrix::BT601, Matrix::BT709, Matrix::BT2020_NCL] {
            for range in [Range::Limited, Range::Full] {
                let (w, h) = (16u32, 8u32);
                let c = conv(m, range, w, h);

                // Planes chosen in the encoded domain so the RGB is in gamut.
                let luma: Vec<u8> = (0..w * h)
                    .map(|i| match range {
                        Range::Limited => 60 + (i * 7 % 120) as u8,
                        Range::Full => 50 + (i * 11 % 150) as u8,
                    })
                    .collect();
                let cb: Vec<u8> = (0..(w / 2) * (h / 2)).map(|i| 120 + (i % 16) as u8).collect();
                let cr: Vec<u8> = (0..(w / 2) * (h / 2)).map(|i| 124 + (i * 3 % 12) as u8).collect();
                let f = |v: &[u8]| v.iter().map(|b| f32::from(*b) / 255.0).collect::<Vec<_>>();
                let rgb = yuv_to_rgb_cpu(&c, &f(&luma), &f(&cb), &f(&cr));
                let ([y2, u2, v2], clipped) = rgb_to_yuv8_cpu(&c, &rgb);
                assert_eq!(clipped, 0);
                let close = |a: &[u8], b: &[u8]| a.iter().zip(b).all(|(p, q)| p.abs_diff(*q) <= 1);
                assert!(close(&luma, &y2), "{m:?} {range:?} luma");
                assert!(close(&cb, &u2), "{m:?} {range:?} cb");
                assert!(close(&cr, &v2), "{m:?} {range:?} cr");
            }
        }
    }

    fn conv(matrix: Matrix, range: Range, w: u32, h: u32) -> YuvConversion {
        YuvConversion {
            choice: MatrixChoice::Declared { code: 1, matrix },
            range,
            width: w,
            height: h,
            chroma_width: w / 2,
            chroma_height: h / 2,
        }
    }

    /// Neutral chroma must produce exact grey under every matrix, or the
    /// conversion tints the whole picture.
    #[test]
    fn neutral_chroma_is_grey_under_every_matrix() {
        for m in [Matrix::BT601, Matrix::BT709, Matrix::BT2020_NCL] {
            for range in [Range::Limited, Range::Full] {
                let c = conv(m, range, 2, 2);
                let (yo, yg, _, _) = range.normalisation();
                // A mid-grey luma with neutral chroma.
                let y = vec![0.5f32; 4];
                let u = vec![128.0 / 255.0; 1];
                let v = vec![128.0 / 255.0; 1];
                let out = yuv_to_rgb_cpu(&c, &y, &u, &v);
                let expect = (0.5 - yo) * yg;
                for px in out.chunks(4) {
                    assert!(
                        (px[0] - expect).abs() < 1e-5
                            && (px[1] - expect).abs() < 1e-5
                            && (px[2] - expect).abs() < 1e-5,
                        "{m:?} {range:?} tinted neutral grey: {px:?}, expected {expect}"
                    );
                }
            }
        }
    }

    /// Limited range must map studio black and white to 0 and 1.
    #[test]
    fn limited_range_maps_studio_black_and_white_to_zero_and_one() {
        let c = conv(Matrix::BT709, Range::Limited, 2, 2);
        let neutral = vec![128.0 / 255.0; 1];

        let black = yuv_to_rgb_cpu(&c, &[16.0 / 255.0; 4], &neutral, &neutral);
        assert!(black[0].abs() < 1e-5, "studio black is not 0: {}", black[0]);

        let white = yuv_to_rgb_cpu(&c, &[235.0 / 255.0; 4], &neutral, &neutral);
        assert!(
            (white[0] - 1.0).abs() < 1e-5,
            "studio white is not 1: {}",
            white[0]
        );
    }

    /// The matrices must actually differ, or parameterising them is theatre.
    #[test]
    fn the_matrices_produce_genuinely_different_colour() {
        let neutral_y = vec![0.5f32; 4];
        let cb = vec![0.8f32; 1];
        let cr = vec![0.3f32; 1];

        let a = yuv_to_rgb_cpu(
            &conv(Matrix::BT601, Range::Limited, 2, 2),
            &neutral_y,
            &cb,
            &cr,
        );
        let b = yuv_to_rgb_cpu(
            &conv(Matrix::BT709, Range::Limited, 2, 2),
            &neutral_y,
            &cb,
            &cr,
        );
        let diff = (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0f32, f32::max);
        assert!(
            diff > 0.02,
            "BT.601 and BT.709 produced nearly identical colour ({diff}); \
             choosing the wrong one would be undetectable, which cannot be right"
        );
    }

    /// A declared code point must win, and an absent one must be marked.
    #[test]
    fn an_assumed_matrix_is_never_reported_as_declared() {
        let declared = MatrixChoice::resolve(Some(1), 1080);
        assert!(!declared.is_assumed());
        assert_eq!(declared.matrix(), Matrix::BT709);
        assert!(declared.describe().contains("declared"));

        let assumed = MatrixChoice::resolve(None, 1080);
        assert!(
            assumed.is_assumed(),
            "an absent code point must be marked assumed"
        );
        assert!(
            assumed.describe().contains("ASSUMED"),
            "the description must make the assumption visible: {}",
            assumed.describe()
        );

        // SD falls back to BT.601, HD to BT.709.
        assert_eq!(MatrixChoice::resolve(None, 480).matrix(), Matrix::BT601);
        assert_eq!(MatrixChoice::resolve(None, 1080).matrix(), Matrix::BT709);

        // BT.2020 is never assumed, at any resolution.
        for h in [480, 720, 1080, 2160, 4320] {
            assert_ne!(
                MatrixChoice::resolve(None, h).matrix(),
                Matrix::BT2020_NCL,
                "BT.2020 must never be assumed; at {h} lines it was"
            );
        }
    }

    /// An unimplemented code point must not silently become a neighbour.
    #[test]
    fn an_unknown_code_point_is_not_substituted() {
        assert!(
            Matrix::from_h273(0).is_none(),
            "identity/RGB is not a YCbCr matrix"
        );
        assert!(Matrix::from_h273(3).is_none(), "reserved");
        assert!(Matrix::from_h273(200).is_none());
        // Falling back is fine; claiming it was declared is not.
        assert!(MatrixChoice::resolve(Some(200), 1080).is_assumed());
    }
}
