//! Video scopes: histogram, waveform, RGB parade, vectorscope and HDR scopes
//! calibrated in nits. All scopes take an interleaved f32 image.

use crate::transfer::{hlg_eotf_nits, nits_to_pq, pq_to_nits};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// A borrowed interleaved image (`channels` ≥ 3, row-major).
#[derive(Clone, Copy, Debug)]
pub struct ImageView<'a> {
    pub data: &'a [f32],
    pub width: usize,
    pub height: usize,
    pub channels: usize,
}

impl<'a> ImageView<'a> {
    /// Wrap a buffer; panics-free (a short buffer yields fewer rows).
    pub fn new(data: &'a [f32], width: usize, height: usize, channels: usize) -> Self {
        ImageView {
            data,
            width,
            height,
            channels,
        }
    }

    /// RGB of a pixel (zeros outside the buffer).
    pub fn pixel(&self, x: usize, y: usize) -> [f32; 3] {
        let i = (y * self.width + x) * self.channels;
        if self.channels < 3 || i + 2 >= self.data.len() {
            return [0.0; 3];
        }
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    /// Number of complete pixels in the buffer.
    pub fn pixel_count(&self) -> usize {
        self.data
            .len()
            .checked_div(self.channels)
            .map_or(0, |n| n.min(self.width * self.height))
    }

    fn rows(&self) -> impl ParallelIterator<Item = (usize, &'a [f32])> + 'a {
        let row_len = self.width * self.channels;
        self.data.par_chunks(row_len).take(self.height).enumerate()
    }
}

/// Rec.709 luma.
#[inline]
pub fn luma_709(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

/// Rec.2020 luma.
#[inline]
pub fn luma_2020(rgb: [f32; 3]) -> f32 {
    0.2627 * rgb[0] + 0.6780 * rgb[1] + 0.0593 * rgb[2]
}

fn bin_of(v: f32, lo: f32, hi: f32, bins: usize) -> usize {
    let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    ((t * bins as f32) as usize).min(bins - 1)
}

// ---------------------------------------------------------------------------
// Histogram
// ---------------------------------------------------------------------------

/// Per-channel + luma histograms.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Histogram {
    pub bins: usize,
    /// Value range covered by the bins.
    pub range: [f32; 2],
    pub r: Vec<u32>,
    pub g: Vec<u32>,
    pub b: Vec<u32>,
    pub luma: Vec<u32>,
    pub pixels: usize,
}

impl Histogram {
    /// Bin index → representative value (bin center).
    pub fn bin_value(&self, i: usize) -> f32 {
        self.range[0] + (i as f32 + 0.5) / self.bins as f32 * (self.range[1] - self.range[0])
    }

    /// Percentile (0–100) of the luma histogram.
    pub fn luma_percentile(&self, p: f32) -> f32 {
        let target = (p.clamp(0.0, 100.0) / 100.0 * self.pixels as f32) as u32;
        let mut acc = 0u32;
        for (i, c) in self.luma.iter().enumerate() {
            acc += c;
            if acc >= target {
                return self.bin_value(i);
            }
        }
        self.range[1]
    }
}

/// Histogram over `[0, 1]` (values outside land in the end bins).
pub fn histogram(img: ImageView<'_>, bins: usize) -> Histogram {
    histogram_range(img, bins, [0.0, 1.0])
}

/// Histogram over an explicit value range.
pub fn histogram_range(img: ImageView<'_>, bins: usize, range: [f32; 2]) -> Histogram {
    let bins = bins.max(1);
    let (lo, hi) = (range[0], range[1].max(range[0] + 1e-6));
    let ch = img.channels.max(1);
    let n = img.pixel_count();
    let acc = img.data[..n * ch]
        .par_chunks(ch * 8192)
        .map(|block| {
            let mut h = vec![0u32; bins * 4];
            for px in block.chunks_exact(ch) {
                let rgb = [px[0], px[1], px[2]];
                h[bin_of(rgb[0], lo, hi, bins)] += 1;
                h[bins + bin_of(rgb[1], lo, hi, bins)] += 1;
                h[2 * bins + bin_of(rgb[2], lo, hi, bins)] += 1;
                h[3 * bins + bin_of(luma_709(rgb), lo, hi, bins)] += 1;
            }
            h
        })
        .reduce(
            || vec![0u32; bins * 4],
            |mut a, b| {
                for (x, y) in a.iter_mut().zip(b) {
                    *x += y;
                }
                a
            },
        );
    Histogram {
        bins,
        range: [lo, hi],
        r: acc[..bins].to_vec(),
        g: acc[bins..2 * bins].to_vec(),
        b: acc[2 * bins..3 * bins].to_vec(),
        luma: acc[3 * bins..].to_vec(),
        pixels: n,
    }
}

// ---------------------------------------------------------------------------
// Waveform / parade
// ---------------------------------------------------------------------------

/// Column-wise value distribution. `luma[col * rows + row]` counts pixels of
/// the image column band whose value falls in the row band; row 0 is the
/// **bottom** (value `range[0]`), row `rows-1` the top.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Waveform {
    pub columns: usize,
    pub rows: usize,
    pub range: [f32; 2],
    pub luma: Vec<u32>,
    pub r: Vec<u32>,
    pub g: Vec<u32>,
    pub b: Vec<u32>,
}

impl Waveform {
    /// Count at (column, row) of a plane.
    pub fn at(plane: &[u32], rows: usize, col: usize, row: usize) -> u32 {
        plane.get(col * rows + row).copied().unwrap_or(0)
    }

    /// Row index → value.
    pub fn row_value(&self, row: usize) -> f32 {
        self.range[0] + (row as f32 + 0.5) / self.rows as f32 * (self.range[1] - self.range[0])
    }
}

fn waveform_with(
    img: ImageView<'_>,
    columns: usize,
    rows: usize,
    range: [f32; 2],
    value: impl Fn([f32; 3]) -> [f32; 4] + Sync,
) -> Waveform {
    let columns = columns.max(1);
    let rows = rows.max(1);
    let (lo, hi) = (range[0], range[1].max(range[0] + 1e-6));
    let w = img.width.max(1);
    let ch = img.channels.max(1);
    let plane = columns * rows;
    let acc = img
        .rows()
        .map(|(_, row)| {
            let mut h = vec![0u32; plane * 4];
            for (x, px) in row.chunks_exact(ch).enumerate() {
                let col = (x * columns / w).min(columns - 1);
                let v = value([px[0], px[1], px[2]]);
                for (p, val) in v.iter().enumerate() {
                    h[p * plane + col * rows + bin_of(*val, lo, hi, rows)] += 1;
                }
            }
            h
        })
        .reduce(
            || vec![0u32; plane * 4],
            |mut a, b| {
                for (x, y) in a.iter_mut().zip(b) {
                    *x += y;
                }
                a
            },
        );
    Waveform {
        columns,
        rows,
        range: [lo, hi],
        luma: acc[..plane].to_vec(),
        r: acc[plane..2 * plane].to_vec(),
        g: acc[2 * plane..3 * plane].to_vec(),
        b: acc[3 * plane..].to_vec(),
    }
}

/// Luma + RGB waveform over `[0, 1]`.
pub fn waveform(img: ImageView<'_>, columns: usize, rows: usize) -> Waveform {
    waveform_with(img, columns, rows, [0.0, 1.0], |rgb| {
        [luma_709(rgb), rgb[0], rgb[1], rgb[2]]
    })
}

/// RGB parade: three side-by-side waveforms (R, G, B), each `columns` wide.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Parade {
    pub columns: usize,
    pub rows: usize,
    pub range: [f32; 2],
    pub r: Vec<u32>,
    pub g: Vec<u32>,
    pub b: Vec<u32>,
}

impl Parade {
    /// Total width when drawn side by side.
    pub fn total_columns(&self) -> usize {
        self.columns * 3
    }
}

/// RGB parade over `[0, 1]`.
pub fn rgb_parade(img: ImageView<'_>, columns: usize, rows: usize) -> Parade {
    let w = waveform(img, columns, rows);
    Parade {
        columns: w.columns,
        rows: w.rows,
        range: w.range,
        r: w.r,
        g: w.g,
        b: w.b,
    }
}

// ---------------------------------------------------------------------------
// Vectorscope
// ---------------------------------------------------------------------------

/// A graticule target (name + position in scope pixels).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub name: String,
    pub x: f32,
    pub y: f32,
}

/// Cb/Cr density plot. `density[y * size + x]`, Cb on x (blue right), Cr on y
/// (red **up**, i.e. y grows downward = negative Cr, as on a hardware scope).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vectorscope {
    pub size: usize,
    pub density: Vec<u32>,
    /// 75 % color bar targets (R, Mg, B, Cy, G, Yl) plus 100 % versions.
    pub targets: Vec<Target>,
    /// Gain applied to Cb/Cr (1.0 = 100 % bars reach the edge).
    pub gain: f32,
}

/// Rec.709 Cb/Cr of an RGB value (Y'CbCr scaling, range ±0.5 for 100 % bars).
#[inline]
pub fn cbcr_709(rgb: [f32; 3]) -> (f32, f32) {
    let y = luma_709(rgb);
    ((rgb[2] - y) / 1.8556, (rgb[0] - y) / 1.5748)
}

fn scope_xy(cb: f32, cr: f32, size: usize, gain: f32) -> (f32, f32) {
    let half = size as f32 * 0.5;
    (
        half + cb * gain * size as f32,
        half - cr * gain * size as f32,
    )
}

/// Vectorscope with a `size × size` plot.
pub fn vectorscope(img: ImageView<'_>, size: usize) -> Vectorscope {
    let size = size.max(2);
    let gain = 1.0;
    let ch = img.channels.max(1);
    let n = img.pixel_count();
    let density = img.data[..n * ch]
        .par_chunks(ch * 8192)
        .map(|block| {
            let mut d = vec![0u32; size * size];
            for px in block.chunks_exact(ch) {
                let (cb, cr) = cbcr_709([px[0], px[1], px[2]]);
                let (x, y) = scope_xy(cb, cr, size, gain);
                let xi = (x.floor().max(0.0) as usize).min(size - 1);
                let yi = (y.floor().max(0.0) as usize).min(size - 1);
                d[yi * size + xi] += 1;
            }
            d
        })
        .reduce(
            || vec![0u32; size * size],
            |mut a, b| {
                for (x, y) in a.iter_mut().zip(b) {
                    *x += y;
                }
                a
            },
        );
    let mut targets = Vec::new();
    for (amp, suffix) in [(0.75f32, "75"), (1.0, "100")] {
        for (name, rgb) in [
            ("R", [1.0, 0.0, 0.0]),
            ("Mg", [1.0, 0.0, 1.0]),
            ("B", [0.0, 0.0, 1.0]),
            ("Cy", [0.0, 1.0, 1.0]),
            ("G", [0.0, 1.0, 0.0]),
            ("Yl", [1.0, 1.0, 0.0]),
        ] {
            let (cb, cr) = cbcr_709([rgb[0] * amp, rgb[1] * amp, rgb[2] * amp]);
            let (x, y) = scope_xy(cb, cr, size, gain);
            targets.push(Target {
                name: format!("{name}{suffix}"),
                x,
                y,
            });
        }
    }
    Vectorscope {
        size,
        density,
        targets,
        gain,
    }
}

// ---------------------------------------------------------------------------
// HDR scopes
// ---------------------------------------------------------------------------

/// HDR waveform with a PQ-spaced vertical axis in nits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HdrWaveform {
    pub columns: usize,
    pub rows: usize,
    /// Luminance (nits) distribution, `[col * rows + row]`, row 0 = 0 nits.
    pub luma: Vec<u32>,
    pub r: Vec<u32>,
    pub g: Vec<u32>,
    pub b: Vec<u32>,
    /// Nits at the center of each row (PQ-spaced, 0 … 10 000).
    pub row_nits: Vec<f32>,
    /// Reference markers (nits, row): 100, 203 (HDR reference white), 1000 and the display peak.
    pub markers: Vec<(f32, usize)>,
    pub peak_nits: f32,
}

/// Row on a PQ-spaced axis for a luminance in nits.
pub fn nits_to_row(nits: f32, rows: usize) -> usize {
    let t = nits_to_pq(nits.max(0.0) as f64) as f32;
    ((t * rows as f32) as usize).min(rows.saturating_sub(1))
}

/// Decode a pixel to display light in nits: PQ (`pq = true`) or HLG at `peak_nits`.
pub fn pixel_nits(rgb: [f32; 3], peak_nits: f32, pq: bool) -> [f32; 3] {
    if pq {
        [
            pq_to_nits(rgb[0] as f64) as f32,
            pq_to_nits(rgb[1] as f64) as f32,
            pq_to_nits(rgb[2] as f64) as f32,
        ]
    } else {
        let n = hlg_eotf_nits(
            [rgb[0] as f64, rgb[1] as f64, rgb[2] as f64],
            peak_nits as f64,
        );
        [n[0] as f32, n[1] as f32, n[2] as f32]
    }
}

/// HDR waveform: code values are decoded through the PQ EOTF (0–10 000 nits)
/// or the HLG EOTF for `peak_nits`; the vertical axis is PQ-spaced.
pub fn hdr_waveform_nits(
    img: ImageView<'_>,
    columns: usize,
    rows: usize,
    peak_nits: f32,
    pq: bool,
) -> HdrWaveform {
    let rows = rows.max(1);
    let w = waveform_with(img, columns, rows, [0.0, 1.0], |rgb| {
        let n = pixel_nits(rgb, peak_nits, pq);
        let y = luma_2020(n);
        [
            nits_to_pq(y as f64) as f32,
            nits_to_pq(n[0] as f64) as f32,
            nits_to_pq(n[1] as f64) as f32,
            nits_to_pq(n[2] as f64) as f32,
        ]
    });
    let row_nits = (0..rows)
        .map(|r| pq_to_nits(((r as f32 + 0.5) / rows as f32) as f64) as f32)
        .collect();
    let mut markers: Vec<(f32, usize)> = [100.0f32, 203.0, 1000.0]
        .iter()
        .map(|n| (*n, nits_to_row(*n, rows)))
        .collect();
    if !markers.iter().any(|(n, _)| (*n - peak_nits).abs() < 1e-3) {
        markers.push((peak_nits, nits_to_row(peak_nits, rows)));
    }
    HdrWaveform {
        columns: w.columns,
        rows,
        luma: w.luma,
        r: w.r,
        g: w.g,
        b: w.b,
        row_nits,
        markers,
        peak_nits,
    }
}

/// Clipping statistics in nits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ClippingStats {
    /// Percentage of pixels whose max channel exceeds 1000 nits.
    pub above_1000_pct: f32,
    /// Percentage of pixels whose max channel exceeds the display peak.
    pub above_peak_pct: f32,
    /// Percentage of pixels whose luminance is below 0.005 nits.
    pub below_0_005_pct: f32,
    pub max_nits: f32,
    pub min_nits: f32,
    pub mean_nits: f32,
    pub pixels: usize,
}

/// Percentages of pixels above 1000 nits / the display peak and below 0.005 nits.
pub fn clipping_stats(img: ImageView<'_>, peak_nits: f32, pq: bool) -> ClippingStats {
    let ch = img.channels.max(1);
    let n = img.pixel_count();
    if n == 0 {
        return ClippingStats::default();
    }
    let (above1000, above_peak, below, max, min, sum) = img.data[..n * ch]
        .par_chunks(ch * 8192)
        .map(|block| {
            let (mut a, mut ap, mut b, mut mx, mut mn, mut s) =
                (0usize, 0usize, 0usize, f32::MIN, f32::MAX, 0f64);
            for px in block.chunks_exact(ch) {
                let nits = pixel_nits([px[0], px[1], px[2]], peak_nits, pq);
                let m = nits[0].max(nits[1]).max(nits[2]);
                let y = luma_2020(nits);
                if m > 1000.0 {
                    a += 1;
                }
                if m > peak_nits {
                    ap += 1;
                }
                if y < 0.005 {
                    b += 1;
                }
                mx = mx.max(m);
                mn = mn.min(y);
                s += y as f64;
            }
            (a, ap, b, mx, mn, s)
        })
        .reduce(
            || (0, 0, 0, f32::MIN, f32::MAX, 0.0),
            |x, y| {
                (
                    x.0 + y.0,
                    x.1 + y.1,
                    x.2 + y.2,
                    x.3.max(y.3),
                    x.4.min(y.4),
                    x.5 + y.5,
                )
            },
        );
    ClippingStats {
        above_1000_pct: above1000 as f32 / n as f32 * 100.0,
        above_peak_pct: above_peak as f32 / n as f32 * 100.0,
        below_0_005_pct: below as f32 / n as f32 * 100.0,
        max_nits: max,
        min_nits: min,
        mean_nits: (sum / n as f64) as f32,
        pixels: n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bars(w: usize, h: usize, amp: f32) -> Vec<f32> {
        let cols: [[f32; 3]; 8] = [
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0],
        ];
        let mut img = vec![0f32; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let c = cols[x * 8 / w];
                let i = (y * w + x) * 3;
                img[i] = c[0] * amp;
                img[i + 1] = c[1] * amp;
                img[i + 2] = c[2] * amp;
            }
        }
        img
    }

    #[test]
    fn histogram_of_ramp() {
        let w = 256;
        let mut img = vec![0f32; w * 4 * 3];
        for y in 0..4 {
            for x in 0..w {
                let v = x as f32 / 255.0;
                let i = (y * w + x) * 3;
                img[i] = v;
                img[i + 1] = v;
                img[i + 2] = v;
            }
        }
        let h = histogram(ImageView::new(&img, w, 4, 3), 16);
        assert_eq!(h.pixels, w * 4);
        assert_eq!(h.luma.iter().sum::<u32>(), (w * 4) as u32);
        for c in &h.luma {
            assert_eq!(*c, 64, "{:?}", h.luma);
        }
        assert!((h.luma_percentile(50.0) - 0.5).abs() < 0.07);
    }

    #[test]
    fn waveform_of_bars() {
        let (w, h) = (80, 10);
        let img = bars(w, h, 0.75);
        let wf = waveform(ImageView::new(&img, w, h, 3), 8, 100);
        // white bar column 0: luma 0.75 → row 75
        assert_eq!(Waveform::at(&wf.luma, wf.rows, 0, 75), (h * 10) as u32);
        // black column 7: row 0
        assert_eq!(Waveform::at(&wf.luma, wf.rows, 7, 0), (h * 10) as u32);
        // red bar column 5: r at 0.75, g at 0
        assert_eq!(Waveform::at(&wf.r, wf.rows, 5, 75), (h * 10) as u32);
        assert_eq!(Waveform::at(&wf.g, wf.rows, 5, 0), (h * 10) as u32);
        let parade = rgb_parade(ImageView::new(&img, w, h, 3), 8, 100);
        assert_eq!(parade.total_columns(), 24);
        assert_eq!(Waveform::at(&parade.b, parade.rows, 6, 75), (h * 10) as u32);
    }

    #[test]
    fn vectorscope_targets_match_bars() {
        let (w, h) = (80, 4);
        let img = bars(w, h, 0.75);
        let vs = vectorscope(ImageView::new(&img, w, h, 3), 256);
        assert_eq!(vs.density.iter().sum::<u32>(), (w * h) as u32);
        // every 75 % target must sit on a populated cell
        for t in vs.targets.iter().filter(|t| t.name.ends_with("75")) {
            let xi = (t.x.floor() as usize).min(255);
            let yi = (t.y.floor() as usize).min(255);
            assert!(
                vs.density[yi * 256 + xi] > 0,
                "target {} at ({xi},{yi}) empty",
                t.name
            );
        }
        // neutral pixels land in the center
        let center = vs.density[128 * 256 + 128];
        assert_eq!(center, (w * h / 4) as u32);
        // red is up (small y), blue is right (large x)
        let r = vs.targets.iter().find(|t| t.name == "R75").unwrap();
        let b = vs.targets.iter().find(|t| t.name == "B75").unwrap();
        assert!(r.y < 128.0 && b.x > 128.0);
    }

    #[test]
    fn hdr_nits_mapping() {
        assert!((pq_to_nits(0.5) - 92.3).abs() < 0.5);
        let px = pixel_nits([0.5, 0.5, 0.5], 1000.0, true);
        assert!((px[1] - 92.3).abs() < 0.5);
        let (w, h) = (16, 2);
        let mut img = vec![0.5f32; w * h * 3];
        // last column: PQ code for 2000 nits
        let hot = nits_to_pq(2000.0) as f32;
        for y in 0..h {
            let i = (y * w + (w - 1)) * 3;
            img[i] = hot;
            img[i + 1] = hot;
            img[i + 2] = hot;
        }
        let wf = hdr_waveform_nits(ImageView::new(&img, w, h, 3), 16, 200, 1000.0, true);
        assert_eq!(wf.markers.len(), 3);
        let row_92 = nits_to_row(92.3, 200);
        assert_eq!(Waveform::at(&wf.luma, wf.rows, 0, row_92), h as u32);
        assert!((wf.row_nits[nits_to_row(100.0, 200)] - 100.0).abs() < 8.0);
        let stats = clipping_stats(ImageView::new(&img, w, h, 3), 1000.0, true);
        assert!(
            (stats.above_1000_pct - 100.0 / 16.0).abs() < 1e-3,
            "{stats:?}"
        );
        assert!((stats.max_nits - 2000.0).abs() < 1.0);
        assert_eq!(stats.below_0_005_pct, 0.0);
        // HLG: 75 % code ≈ 203 nits on a 1000-nit display
        let px = pixel_nits([0.75, 0.75, 0.75], 1000.0, false);
        assert!((px[1] - 203.0).abs() < 5.0, "{px:?}");
        let dark = vec![0.0f32; 12];
        let s = clipping_stats(ImageView::new(&dark, 2, 2, 3), 1000.0, true);
        assert_eq!(s.below_0_005_pct, 100.0);
    }
}
