//! Spatial panning: stereo pan laws, cinema channel layouts, vector base
//! amplitude panning (Pulkki), object panning and downmix matrices.
//!
//! Coordinate frame (right-handed, listener at the origin):
//! * `x` forward, `y` left, `z` up.
//! * Azimuth in degrees, `0` = front, positive = counter-clockwise (left) when
//!   seen from above (ITU-R BS.2051 convention: `L = +30°`, `R = -30°`).
//! * Elevation in degrees, positive = up.

use crate::buffer::{db_to_lin, AudioBuffer};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Stereo pan laws
// ---------------------------------------------------------------------------

/// Stereo pan law: the attenuation applied to each side at center.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PanLaw {
    /// Constant power (sin/cos), −3 dB at center.
    #[default]
    ConstantPower3dB,
    /// Compromise law, −4.5 dB at center (geometric mean of the other two).
    Minus4_5dB,
    /// Linear, −6 dB at center (constant amplitude sum).
    Minus6dB,
}

impl PanLaw {
    /// Left/right gains for a pan position `pan ∈ [-1, 1]` (−1 = hard left).
    pub fn gains(&self, pan: f32) -> (f32, f32) {
        let p = pan.clamp(-1.0, 1.0);
        let theta = (p + 1.0) * 0.5 * std::f32::consts::FRAC_PI_2;
        let (cp_l, cp_r) = (theta.cos(), theta.sin());
        let (lin_l, lin_r) = ((1.0 - p) * 0.5, (1.0 + p) * 0.5);
        match self {
            PanLaw::ConstantPower3dB => (cp_l, cp_r),
            PanLaw::Minus6dB => (lin_l, lin_r),
            PanLaw::Minus4_5dB => ((cp_l * lin_l).sqrt(), (cp_r * lin_r).sqrt()),
        }
    }

    /// Center attenuation in dB (negative).
    pub fn center_db(&self) -> f32 {
        let (l, _) = self.gains(0.0);
        20.0 * l.log10()
    }
}

/// Stereo constant-power pan gains (convenience for the default law).
pub fn stereo_gains(pan: f32, law: PanLaw) -> (f32, f32) {
    law.gains(pan)
}

// ---------------------------------------------------------------------------
// Layouts
// ---------------------------------------------------------------------------

/// One loudspeaker of a layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Speaker {
    /// Short channel label (`L`, `Rs`, `Ltf` …).
    pub name: &'static str,
    /// Azimuth in degrees (positive = left).
    pub azimuth_deg: f32,
    /// Elevation in degrees (positive = up).
    pub elevation_deg: f32,
    /// True for the low-frequency-effects channel (never panned to).
    pub lfe: bool,
}

impl Speaker {
    const fn new(name: &'static str, azimuth_deg: f32, elevation_deg: f32) -> Self {
        Speaker {
            name,
            azimuth_deg,
            elevation_deg,
            lfe: false,
        }
    }
    const fn lfe() -> Self {
        Speaker {
            name: "LFE",
            azimuth_deg: 0.0,
            elevation_deg: -90.0,
            lfe: true,
        }
    }
    /// Unit direction vector.
    pub fn direction(&self) -> [f32; 3] {
        direction_from_angles(self.azimuth_deg, self.elevation_deg)
    }
}

/// Speaker layouts supported by the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChannelLayout {
    Mono,
    #[default]
    Stereo,
    /// Left / Center / Right.
    Lcr,
    /// `L R C LFE Ls Rs` (ITU-R BS.775).
    Surround51,
    /// `L R C LFE Lss Rss Lrs Rrs`.
    Surround71,
    /// 7.1 bed plus four height channels `Ltf Rtf Ltb Rtb` (Atmos bed).
    Atmos714,
}

const MONO: &[Speaker] = &[Speaker::new("M", 0.0, 0.0)];
const STEREO: &[Speaker] = &[Speaker::new("L", 30.0, 0.0), Speaker::new("R", -30.0, 0.0)];
const LCR: &[Speaker] = &[
    Speaker::new("L", 30.0, 0.0),
    Speaker::new("C", 0.0, 0.0),
    Speaker::new("R", -30.0, 0.0),
];
const S51: &[Speaker] = &[
    Speaker::new("L", 30.0, 0.0),
    Speaker::new("R", -30.0, 0.0),
    Speaker::new("C", 0.0, 0.0),
    Speaker::lfe(),
    Speaker::new("Ls", 110.0, 0.0),
    Speaker::new("Rs", -110.0, 0.0),
];
const S71: &[Speaker] = &[
    Speaker::new("L", 30.0, 0.0),
    Speaker::new("R", -30.0, 0.0),
    Speaker::new("C", 0.0, 0.0),
    Speaker::lfe(),
    Speaker::new("Lss", 90.0, 0.0),
    Speaker::new("Rss", -90.0, 0.0),
    Speaker::new("Lrs", 135.0, 0.0),
    Speaker::new("Rrs", -135.0, 0.0),
];
const S714: &[Speaker] = &[
    Speaker::new("L", 30.0, 0.0),
    Speaker::new("R", -30.0, 0.0),
    Speaker::new("C", 0.0, 0.0),
    Speaker::lfe(),
    Speaker::new("Lss", 90.0, 0.0),
    Speaker::new("Rss", -90.0, 0.0),
    Speaker::new("Lrs", 135.0, 0.0),
    Speaker::new("Rrs", -135.0, 0.0),
    Speaker::new("Ltf", 45.0, 45.0),
    Speaker::new("Rtf", -45.0, 45.0),
    Speaker::new("Ltb", 135.0, 45.0),
    Speaker::new("Rtb", -135.0, 45.0),
];

/// Channels in the widest supported layout (7.1.4). The gain paths that must
/// not allocate use fixed buffers of this size; `layouts_fit_max_channels`
/// keeps the constant honest.
pub const MAX_CHANNELS: usize = 12;

impl ChannelLayout {
    /// All layouts, smallest first.
    pub const ALL: [ChannelLayout; 6] = [
        ChannelLayout::Mono,
        ChannelLayout::Stereo,
        ChannelLayout::Lcr,
        ChannelLayout::Surround51,
        ChannelLayout::Surround71,
        ChannelLayout::Atmos714,
    ];

    /// Speaker table in channel order.
    pub fn speakers(&self) -> &'static [Speaker] {
        match self {
            ChannelLayout::Mono => MONO,
            ChannelLayout::Stereo => STEREO,
            ChannelLayout::Lcr => LCR,
            ChannelLayout::Surround51 => S51,
            ChannelLayout::Surround71 => S71,
            ChannelLayout::Atmos714 => S714,
        }
    }

    /// Number of channels (including LFE).
    pub fn channel_count(&self) -> usize {
        self.speakers().len()
    }

    /// Index of the LFE channel, if any.
    pub fn lfe_index(&self) -> Option<usize> {
        self.speakers().iter().position(|s| s.lfe)
    }

    /// Whether any speaker is elevated (3D panning needed).
    pub fn is_3d(&self) -> bool {
        self.speakers()
            .iter()
            .any(|s| !s.lfe && s.elevation_deg.abs() > 1e-3)
    }

    /// Channel index by label.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.speakers().iter().position(|s| s.name == name)
    }

    /// Layout for a channel count, if one matches.
    pub fn from_channel_count(n: usize) -> Option<ChannelLayout> {
        Self::ALL.iter().copied().find(|l| l.channel_count() == n)
    }

    /// Human label (`5.1`, `7.1.4` …).
    pub fn label(&self) -> &'static str {
        match self {
            ChannelLayout::Mono => "mono",
            ChannelLayout::Stereo => "stereo",
            ChannelLayout::Lcr => "LCR",
            ChannelLayout::Surround51 => "5.1",
            ChannelLayout::Surround71 => "7.1",
            ChannelLayout::Atmos714 => "7.1.4",
        }
    }
}

/// Unit vector for `(azimuth, elevation)` in degrees.
pub fn direction_from_angles(azimuth_deg: f32, elevation_deg: f32) -> [f32; 3] {
    let az = azimuth_deg.to_radians();
    let el = elevation_deg.to_radians();
    [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()]
}

/// `(azimuth, elevation)` in degrees for a direction vector.
pub fn angles_from_direction(v: [f32; 3]) -> (f32, f32) {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-9 {
        return (0.0, 0.0);
    }
    let az = v[1].atan2(v[0]).to_degrees();
    let el = (v[2] / len).clamp(-1.0, 1.0).asin().to_degrees();
    (az, el)
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = dot(v, v).sqrt();
    if len < 1e-9 {
        [1.0, 0.0, 0.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

// ---------------------------------------------------------------------------
// VBAP
// ---------------------------------------------------------------------------

/// Parameters for panning a positioned object.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObjectPanParams {
    /// Distance (in the position's units) at which attenuation starts.
    pub reference_distance: f32,
    /// Distance-attenuation exponent: gain = (ref / d)^rolloff for d > ref.
    pub rolloff: f32,
    /// Angular spread in degrees (0 = point source; 360 = omni).
    pub spread_deg: f32,
    /// LFE send level in dB, or `None` for no LFE contribution.
    pub lfe_send_db: Option<f32>,
}

impl Default for ObjectPanParams {
    fn default() -> Self {
        ObjectPanParams {
            reference_distance: 1.0,
            rolloff: 1.0,
            spread_deg: 0.0,
            lfe_send_db: None,
        }
    }
}

/// A 2D pair or 3D triplet of speakers with its inverse base matrix.
#[derive(Clone, Debug)]
struct Base {
    /// Channel indices (2 for pairs, 3 for triplets; virtual speakers use `usize::MAX`).
    idx: [usize; 3],
    /// Row-major inverse of the 3×3 matrix whose rows are the speaker vectors
    /// (for pairs the third row is the pair's normal, so the math is uniform).
    inv: [[f32; 3]; 3],
}

/// Vector base amplitude panner for a [`ChannelLayout`].
#[derive(Clone, Debug)]
pub struct VbapPanner {
    layout: ChannelLayout,
    bases: Vec<Base>,
    three_d: bool,
    /// Channel indices of the speakers that can be panned to (everything but
    /// the LFE), and their unit directions. Both are fixed by the layout, so
    /// deriving them per call — as this once did, with a `Vec` and a `sin_cos`
    /// per speaker — was work repeated for an answer that never changed.
    real: Vec<usize>,
    real_dirs: Vec<[f32; 3]>,
}

impl VbapPanner {
    /// Build the speaker pairs/triplets for a layout.
    pub fn new(layout: ChannelLayout) -> Self {
        let speakers = layout.speakers();
        let three_d = layout.is_3d();
        let mut dirs: Vec<(usize, [f32; 3])> = speakers
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.lfe)
            .map(|(i, s)| (i, s.direction()))
            .collect();
        let mut bases = Vec::new();
        if dirs.len() == 1 {
            // Single speaker: handled specially in `gains`.
            return VbapPanner::assemble(layout, bases, three_d);
        }
        if !three_d {
            // Sort by azimuth and pair adjacent speakers (wrapping), skipping
            // pairs that span 180° or more.
            dirs.sort_by(|a, b| {
                let aa = a.1[1].atan2(a.1[0]);
                let ab = b.1[1].atan2(b.1[0]);
                aa.partial_cmp(&ab).unwrap_or(std::cmp::Ordering::Equal)
            });
            let n = dirs.len();
            let az: Vec<f32> = dirs
                .iter()
                .map(|(_, d)| d[1].atan2(d[0]).to_degrees())
                .collect();
            for i in 0..n {
                let (ia, a) = dirs[i];
                let (ib, b) = dirs[(i + 1) % n];
                if n == 2 && i == 1 {
                    break;
                }
                // Angular span of the arc between the two speakers; a pair
                // only works for sources inside an arc narrower than 180°.
                let span = if i + 1 < n {
                    az[i + 1] - az[i]
                } else {
                    360.0 - (az[n - 1] - az[0])
                };
                if span <= 1e-3 || span >= 180.0 - 1e-3 {
                    continue;
                }
                let normal = normalize(cross(a, b));
                if let Some(inv) = invert3([a, b, normal]) {
                    bases.push(Base {
                        idx: [ia, ib, usize::MAX],
                        inv,
                    });
                }
            }
        } else {
            // Add a virtual speaker at the nadir so the hull is closed below
            // the horizon, then take every hull face as a triplet.
            let mut pts = dirs.clone();
            pts.push((usize::MAX, [0.0, 0.0, -1.0]));
            if !pts.iter().any(|(_, d)| d[2] > 0.95) {
                // No zenith speaker: fine, the top faces are real triangles.
            }
            let n = pts.len();
            for i in 0..n {
                for j in (i + 1)..n {
                    for k in (j + 1)..n {
                        let (a, b, c) = (pts[i].1, pts[j].1, pts[k].1);
                        let normal = cross(sub(b, a), sub(c, a));
                        if dot(normal, normal) < 1e-9 {
                            continue;
                        }
                        // Face of the convex hull: every other point on the inner side.
                        let mut sign = 0.0f32;
                        let mut is_face = true;
                        for (m, (_, p)) in pts.iter().enumerate() {
                            if m == i || m == j || m == k {
                                continue;
                            }
                            let d = dot(normal, sub(*p, a));
                            if d.abs() < 1e-4 {
                                continue;
                            }
                            if sign == 0.0 {
                                sign = d.signum();
                            } else if d.signum() != sign {
                                is_face = false;
                                break;
                            }
                        }
                        if !is_face {
                            continue;
                        }
                        // The origin must be on the inner side too (it is, for
                        // layouts surrounding the listener).
                        if let Some(inv) = invert3([a, b, c]) {
                            bases.push(Base {
                                idx: [pts[i].0, pts[j].0, pts[k].0],
                                inv,
                            });
                        }
                    }
                }
            }
        }
        VbapPanner::assemble(layout, bases, three_d)
    }

    fn assemble(layout: ChannelLayout, bases: Vec<Base>, three_d: bool) -> Self {
        let speakers = layout.speakers();
        let real: Vec<usize> = (0..speakers.len()).filter(|i| !speakers[*i].lfe).collect();
        let real_dirs = real.iter().map(|i| speakers[*i].direction()).collect();
        VbapPanner {
            layout,
            bases,
            three_d,
            real,
            real_dirs,
        }
    }

    /// The layout this panner targets.
    pub fn layout(&self) -> ChannelLayout {
        self.layout
    }

    /// Number of speaker pairs/triplets in use.
    pub fn base_count(&self) -> usize {
        self.bases.len()
    }

    /// Constant-power gains (one per channel, LFE = 0) for a source direction.
    pub fn gains(&self, azimuth_deg: f32, elevation_deg: f32) -> Vec<f32> {
        self.gains_for_direction(direction_from_angles(azimuth_deg, elevation_deg))
    }

    /// Gains for a unit direction vector.
    pub fn gains_for_direction(&self, dir: [f32; 3]) -> Vec<f32> {
        let mut out = vec![0.0f32; self.layout.channel_count()];
        self.gains_for_direction_into(dir, &mut out);
        out
    }

    /// [`gains`](Self::gains) into a caller-owned slice of
    /// [`channel_count`](ChannelLayout::channel_count) floats.
    pub fn gains_into(&self, azimuth_deg: f32, elevation_deg: f32, out: &mut [f32]) {
        self.gains_for_direction_into(direction_from_angles(azimuth_deg, elevation_deg), out);
    }

    /// [`gains_for_direction`](Self::gains_for_direction) into a caller-owned
    /// slice of [`channel_count`](ChannelLayout::channel_count) floats.
    ///
    /// This is the form the renderer uses. Panning a spread source evaluates
    /// this seventeen to twenty-five times for one control value; returning a
    /// fresh `Vec` from each was most of what that cost.
    pub fn gains_for_direction_into(&self, dir: [f32; 3], out: &mut [f32]) {
        debug_assert_eq!(out.len(), self.layout.channel_count());
        out.fill(0.0);
        if self.real.len() == 1 {
            out[self.real[0]] = 1.0;
            return;
        }
        let mut p = normalize(dir);
        if !self.three_d {
            // Project onto the horizontal plane.
            p = normalize([p[0], p[1], 0.0]);
        }
        // Pick the base with the largest minimum gain.
        let mut best: Option<(f32, [f32; 3], &Base)> = None;
        for base in &self.bases {
            let g = mat_vec(&base.inv, p);
            let count = if base.idx[2] == usize::MAX && !self.three_d {
                2
            } else {
                3
            };
            let min = g[..count].iter().copied().fold(f32::INFINITY, f32::min);
            if best.as_ref().is_none_or(|(m, _, _)| min > *m) {
                best = Some((min, g, base));
            }
        }
        let Some((_, g, base)) = best else {
            // Degenerate layout: nearest speaker.
            self.nearest_into(p, out);
            return;
        };
        let count = if base.idx[2] == usize::MAX && !self.three_d {
            2
        } else {
            3
        };
        let min = g[..count].iter().copied().fold(f32::INFINITY, f32::min);
        if !self.three_d && min < -1e-4 {
            // Outside every speaker arc (e.g. behind a stereo pair): nearest speaker.
            self.nearest_into(p, out);
            return;
        }
        for (ch, gi) in base.idx[..count].iter().zip(&g[..count]) {
            if *ch != usize::MAX {
                out[*ch] = gi.max(0.0);
            }
        }
        normalize_power(out);
        if out.iter().all(|g| *g == 0.0) {
            // Everything went to a virtual speaker (source straight below):
            // spread evenly over the real speakers of that base.
            for i in base.idx[..count].iter().copied() {
                if i != usize::MAX {
                    out[i] = 1.0;
                }
            }
            normalize_power(out);
        }
    }

    /// All the gain on whichever real speaker points most nearly at `p`.
    fn nearest_into(&self, p: [f32; 3], out: &mut [f32]) {
        let nearest = self
            .real
            .iter()
            .copied()
            .zip(&self.real_dirs)
            .max_by(|a, b| {
                dot(*a.1, p)
                    .partial_cmp(&dot(*b.1, p))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i);
        if let Some(i) = nearest {
            out[i] = 1.0;
        }
    }

    /// Gains with angular spread (MDAP): the source is replaced by a ring of
    /// virtual sources around the direction and their power is summed.
    pub fn gains_with_spread(
        &self,
        azimuth_deg: f32,
        elevation_deg: f32,
        spread_deg: f32,
    ) -> Vec<f32> {
        let mut out = vec![0.0f32; self.layout.channel_count()];
        self.gains_with_spread_into(azimuth_deg, elevation_deg, spread_deg, &mut out);
        out
    }

    /// [`gains_with_spread`](Self::gains_with_spread) into a caller-owned slice
    /// of [`channel_count`](ChannelLayout::channel_count) floats.
    pub fn gains_with_spread_into(
        &self,
        azimuth_deg: f32,
        elevation_deg: f32,
        spread_deg: f32,
        out: &mut [f32],
    ) {
        let n = self.layout.channel_count();
        debug_assert_eq!(out.len(), n);
        debug_assert!(n <= MAX_CHANNELS);
        let spread = spread_deg.clamp(0.0, 360.0);
        if spread < 1.0 {
            self.gains_into(azimuth_deg, elevation_deg, out);
            return;
        }
        let mut acc = [0.0f32; MAX_CHANNELS];
        let mut g = [0.0f32; MAX_CHANNELS];
        let center = direction_from_angles(azimuth_deg, elevation_deg);
        // Orthonormal frame around the center direction.
        let up = if center[2].abs() > 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 0.0, 1.0]
        };
        let u = normalize(cross(up, center));
        let v = normalize(cross(center, u));
        let half = (spread * 0.5).to_radians();
        let rings = if spread >= 180.0 { 3 } else { 2 };
        // The eight ring offsets are the same on every ring and in every call;
        // taking their sine and cosine per ring per call was sixteen to
        // twenty-four transcendentals for eight answers.
        let mut ring = [(0.0f32, 0.0f32); 8];
        for (k, e) in ring.iter_mut().enumerate() {
            let phi = k as f32 * std::f32::consts::TAU / 8.0;
            *e = (phi.cos(), phi.sin());
        }
        self.gains_for_direction_into(center, &mut g[..n]);
        for (a, gi) in acc[..n].iter_mut().zip(&g[..n]) {
            *a += gi * gi;
        }
        for r in 1..=rings {
            let ang = half * r as f32 / rings as f32;
            let (s, c) = ang.sin_cos();
            for (cp, sp) in ring {
                let dir = [
                    center[0] * c + (u[0] * cp + v[0] * sp) * s,
                    center[1] * c + (u[1] * cp + v[1] * sp) * s,
                    center[2] * c + (u[2] * cp + v[2] * sp) * s,
                ];
                self.gains_for_direction_into(dir, &mut g[..n]);
                for (a, gi) in acc[..n].iter_mut().zip(&g[..n]) {
                    *a += gi * gi;
                }
            }
        }
        for (o, a) in out.iter_mut().zip(&acc[..n]) {
            *o = a.sqrt();
        }
        normalize_power(out);
    }

    /// Gains for an object at `position` (x forward, y left, z up) with
    /// distance attenuation, spread and an optional LFE send.
    pub fn pan_object(&self, position: [f32; 3], params: &ObjectPanParams) -> Vec<f32> {
        let mut out = vec![0.0f32; self.layout.channel_count()];
        self.pan_object_into(position, params, &mut out);
        out
    }

    /// [`pan_object`](Self::pan_object) into a caller-owned slice of
    /// [`channel_count`](ChannelLayout::channel_count) floats.
    pub fn pan_object_into(&self, position: [f32; 3], params: &ObjectPanParams, out: &mut [f32]) {
        debug_assert_eq!(out.len(), self.layout.channel_count());
        let d = dot(position, position).sqrt();
        let reference = params.reference_distance.max(1e-3);
        let att = if d > reference {
            (reference / d).powf(params.rolloff.max(0.0))
        } else {
            1.0
        };
        if d < 1e-4 {
            // At the listener: omni.
            self.gains_with_spread_into(0.0, 0.0, 360.0, out);
        } else {
            let (az, el) = angles_from_direction(position);
            self.gains_with_spread_into(az, el, params.spread_deg, out);
        }
        for g in out.iter_mut() {
            *g *= att;
        }
        if let (Some(lfe), Some(db)) = (self.layout.lfe_index(), params.lfe_send_db) {
            out[lfe] = att * db_to_lin(db);
        }
    }
}

fn mat_vec(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    // g = p · L⁻¹ where rows of L are the speaker vectors ⇒ g_i = Σ_j p_j inv[j][i]
    [
        v[0] * m[0][0] + v[1] * m[1][0] + v[2] * m[2][0],
        v[0] * m[0][1] + v[1] * m[1][1] + v[2] * m[2][1],
        v[0] * m[0][2] + v[1] * m[1][2] + v[2] * m[2][2],
    ]
}

fn invert3(rows: [[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let m = rows;
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-7 {
        return None;
    }
    let inv_det = 1.0 / det;
    Some([
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv_det,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv_det,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv_det,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv_det,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv_det,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv_det,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv_det,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv_det,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv_det,
        ],
    ])
}

/// Scale gains so their power sum is 1 (no-op for all-zero input).
pub fn normalize_power(gains: &mut [f32]) {
    let p: f32 = gains.iter().map(|g| g * g).sum();
    if p > 1e-12 {
        let s = 1.0 / p.sqrt();
        for g in gains.iter_mut() {
            *g *= s;
        }
    }
}

// ---------------------------------------------------------------------------
// Downmix
// ---------------------------------------------------------------------------

/// A downmix/upmix matrix: `rows[out][in]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Downmix {
    pub from: ChannelLayout,
    pub to: ChannelLayout,
    /// `rows[out_channel][in_channel]` gains.
    pub rows: Vec<Vec<f32>>,
}

const M3: f32 = std::f32::consts::FRAC_1_SQRT_2; // −3 dB

impl Downmix {
    /// Identity matrix.
    fn identity(layout: ChannelLayout) -> Self {
        let n = layout.channel_count();
        let rows = (0..n)
            .map(|o| (0..n).map(|i| if i == o { 1.0 } else { 0.0 }).collect())
            .collect();
        Downmix {
            from: layout,
            to: layout,
            rows,
        }
    }

    /// Build a matrix by naming `(out, in, gain)` entries.
    fn from_entries(from: ChannelLayout, to: ChannelLayout, entries: &[(&str, &str, f32)]) -> Self {
        let mut rows = vec![vec![0.0f32; from.channel_count()]; to.channel_count()];
        for (o, i, g) in entries {
            if let (Some(oi), Some(ii)) = (to.index_of(o), from.index_of(i)) {
                rows[oi][ii] = *g;
            }
        }
        Downmix { from, to, rows }
    }

    /// One step down (or up) the layout ladder.
    fn step(from: ChannelLayout, to: ChannelLayout) -> Option<Self> {
        use ChannelLayout::*;
        Some(match (from, to) {
            (a, b) if a == b => Self::identity(a),
            // ITU-R BS.775 5.1 → stereo.
            (Surround51, Stereo) => Self::from_entries(
                from,
                to,
                &[
                    ("L", "L", 1.0),
                    ("L", "C", M3),
                    ("L", "Ls", M3),
                    ("R", "R", 1.0),
                    ("R", "C", M3),
                    ("R", "Rs", M3),
                ],
            ),
            (Surround71, Surround51) => Self::from_entries(
                from,
                to,
                &[
                    ("L", "L", 1.0),
                    ("R", "R", 1.0),
                    ("C", "C", 1.0),
                    ("LFE", "LFE", 1.0),
                    ("Ls", "Lss", M3),
                    ("Ls", "Lrs", M3),
                    ("Rs", "Rss", M3),
                    ("Rs", "Rrs", M3),
                ],
            ),
            (Atmos714, Surround71) => Self::from_entries(
                from,
                to,
                &[
                    ("L", "L", 1.0),
                    ("R", "R", 1.0),
                    ("C", "C", 1.0),
                    ("LFE", "LFE", 1.0),
                    ("Lss", "Lss", 1.0),
                    ("Rss", "Rss", 1.0),
                    ("Lrs", "Lrs", 1.0),
                    ("Rrs", "Rrs", 1.0),
                    ("L", "Ltf", M3),
                    ("R", "Rtf", M3),
                    ("Lrs", "Ltb", M3),
                    ("Rrs", "Rtb", M3),
                ],
            ),
            (Lcr, Stereo) => Self::from_entries(
                from,
                to,
                &[
                    ("L", "L", 1.0),
                    ("L", "C", M3),
                    ("R", "R", 1.0),
                    ("R", "C", M3),
                ],
            ),
            (Surround51, Lcr) => Self::from_entries(
                from,
                to,
                &[
                    ("L", "L", 1.0),
                    ("L", "Ls", M3),
                    ("C", "C", 1.0),
                    ("R", "R", 1.0),
                    ("R", "Rs", M3),
                ],
            ),
            (Stereo, Mono) => Self::from_entries(from, to, &[("M", "L", 0.5), ("M", "R", 0.5)]),
            // Upmixes (channel placement only).
            (Mono, Stereo) => Self::from_entries(from, to, &[("L", "M", M3), ("R", "M", M3)]),
            (Stereo, Lcr) => Self::from_entries(from, to, &[("L", "L", 1.0), ("R", "R", 1.0)]),
            (Lcr, Surround51) => Self::from_entries(
                from,
                to,
                &[("L", "L", 1.0), ("C", "C", 1.0), ("R", "R", 1.0)],
            ),
            (Surround51, Surround71) => Self::from_entries(
                from,
                to,
                &[
                    ("L", "L", 1.0),
                    ("R", "R", 1.0),
                    ("C", "C", 1.0),
                    ("LFE", "LFE", 1.0),
                    ("Lss", "Ls", 1.0),
                    ("Rss", "Rs", 1.0),
                ],
            ),
            (Surround71, Atmos714) => {
                let names = ["L", "R", "C", "LFE", "Lss", "Rss", "Lrs", "Rrs"];
                let entries: Vec<(&str, &str, f32)> = names.iter().map(|n| (*n, *n, 1.0)).collect();
                Self::from_entries(from, to, &entries)
            }
            _ => return None,
        })
    }

    /// Matrix between any two layouts (composed along the layout ladder).
    pub fn between(from: ChannelLayout, to: ChannelLayout) -> Option<Self> {
        if from == to {
            return Some(Self::identity(from));
        }
        let ladder = ChannelLayout::ALL;
        let fi = ladder.iter().position(|l| *l == from)?;
        let ti = ladder.iter().position(|l| *l == to)?;
        let mut cur = Self::identity(from);
        let mut i = fi;
        while i != ti {
            let next = if ti > i { i + 1 } else { i - 1 };
            let step = Self::step(ladder[i], ladder[next])?;
            cur = step.compose_after(&cur);
            i = next;
        }
        Some(cur)
    }

    /// `self ∘ inner`: apply `inner` first, then `self`.
    fn compose_after(&self, inner: &Downmix) -> Downmix {
        let mut rows = vec![vec![0.0f32; inner.from.channel_count()]; self.to.channel_count()];
        for (o, row) in rows.iter_mut().enumerate() {
            for (i, cell) in row.iter_mut().enumerate() {
                *cell = (0..inner.to.channel_count())
                    .map(|m| self.rows[o][m] * inner.rows[m][i])
                    .sum();
            }
        }
        Downmix {
            from: inner.from,
            to: self.to,
            rows,
        }
    }

    /// Apply the matrix to a buffer whose channel count matches `from`.
    pub fn apply(&self, input: &AudioBuffer) -> Result<AudioBuffer> {
        let fin = self.from.channel_count();
        let fout = self.to.channel_count();
        if input.channels != fin {
            return Err(Error::Mismatch(format!(
                "downmix expects {} channels, buffer has {}",
                fin, input.channels
            )));
        }
        let frames = input.frames();
        let mut out = AudioBuffer::new(input.sample_rate, fout, frames);
        for f in 0..frames {
            let src = input.frame(f);
            for (o, row) in self.rows.iter().enumerate() {
                let mut acc = 0.0f32;
                for (i, g) in row.iter().enumerate() {
                    if *g != 0.0 {
                        acc += g * src[i];
                    }
                }
                out.data[f * fout + o] = acc;
            }
        }
        Ok(out)
    }
}

/// Convert a buffer from one layout to another.
pub fn downmix(input: &AudioBuffer, from: ChannelLayout, to: ChannelLayout) -> Result<AudioBuffer> {
    let m = Downmix::between(from, to).ok_or_else(|| {
        Error::InvalidArgument(format!(
            "no downmix from {} to {}",
            from.label(),
            to.label()
        ))
    })?;
    m.apply(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn power(g: &[f32]) -> f32 {
        g.iter().map(|x| x * x).sum()
    }

    #[test]
    fn pan_laws_center_attenuation() {
        assert!((PanLaw::ConstantPower3dB.center_db() + 3.01).abs() < 0.05);
        assert!((PanLaw::Minus4_5dB.center_db() + 4.5).abs() < 0.1);
        assert!((PanLaw::Minus6dB.center_db() + 6.02).abs() < 0.05);
        let (l, r) = PanLaw::ConstantPower3dB.gains(-1.0);
        assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6);
        let (l, r) = PanLaw::Minus6dB.gains(0.5);
        assert!((l - 0.25).abs() < 1e-6 && (r - 0.75).abs() < 1e-6);
    }

    #[test]
    fn layouts_fit_max_channels() {
        for l in ChannelLayout::ALL {
            assert!(
                l.channel_count() <= MAX_CHANNELS,
                "{} has {} channels, past MAX_CHANNELS = {MAX_CHANNELS}",
                l.label(),
                l.channel_count()
            );
        }
    }

    #[test]
    fn gains_into_agrees_with_the_allocating_form() {
        for l in ChannelLayout::ALL {
            let p = VbapPanner::new(l);
            let n = l.channel_count();
            let mut buf = vec![0.0f32; n];
            for az in [-170.0f32, -90.0, -33.0, 0.0, 17.0, 95.0, 179.0] {
                for (el, spread) in [(0.0f32, 0.0f32), (30.0, 0.0), (0.0, 45.0), (20.0, 200.0)] {
                    p.gains_with_spread_into(az, el, spread, &mut buf);
                    assert_eq!(
                        buf,
                        p.gains_with_spread(az, el, spread),
                        "{l:?} {az} {el} {spread}"
                    );
                }
            }
            p.pan_object_into([1.5, 0.5, 0.2], &ObjectPanParams::default(), &mut buf);
            assert_eq!(
                buf,
                p.pan_object([1.5, 0.5, 0.2], &ObjectPanParams::default())
            );
        }
    }

    #[test]
    fn layouts_have_expected_shapes() {
        assert_eq!(ChannelLayout::Surround51.channel_count(), 6);
        assert_eq!(ChannelLayout::Surround51.lfe_index(), Some(3));
        assert_eq!(ChannelLayout::Atmos714.channel_count(), 12);
        assert!(ChannelLayout::Atmos714.is_3d());
        assert!(!ChannelLayout::Surround71.is_3d());
        assert_eq!(
            ChannelLayout::from_channel_count(8),
            Some(ChannelLayout::Surround71)
        );
        let (az, el) = angles_from_direction(direction_from_angles(110.0, 20.0));
        assert!((az - 110.0).abs() < 1e-3 && (el - 20.0).abs() < 1e-3);
    }

    #[test]
    fn vbap_source_at_speaker_hits_only_that_speaker() {
        for layout in ChannelLayout::ALL {
            let p = VbapPanner::new(layout);
            for (i, s) in layout.speakers().iter().enumerate() {
                if s.lfe {
                    continue;
                }
                let g = p.gains(s.azimuth_deg, s.elevation_deg);
                assert_eq!(g.len(), layout.channel_count());
                assert!(
                    (g[i] - 1.0).abs() < 1e-3,
                    "{}: speaker {} got {:?}",
                    layout.label(),
                    s.name,
                    g
                );
                for (j, gj) in g.iter().enumerate() {
                    if j != i {
                        assert!(
                            gj.abs() < 1e-3,
                            "{}: leak into {} for {}: {:?}",
                            layout.label(),
                            j,
                            s.name,
                            g
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn vbap_midway_between_l_and_r_is_equal_and_constant_power() {
        let p = VbapPanner::new(ChannelLayout::Stereo);
        let g = p.gains(0.0, 0.0);
        assert!((g[0] - g[1]).abs() < 1e-5);
        assert!((power(&g) - 1.0).abs() < 1e-4);
        assert!((g[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4);
        // Hard left beyond the speaker arc collapses to L.
        let g = p.gains(90.0, 0.0);
        assert!((g[0] - 1.0).abs() < 1e-4 && g[1].abs() < 1e-5);
        // 5.1: midway between L (30) and Ls (110) shares power between them only.
        let p = VbapPanner::new(ChannelLayout::Surround51);
        let g = p.gains(70.0, 0.0);
        assert!((g[0] - g[4]).abs() < 1e-4, "{g:?}");
        assert!(g[1].abs() < 1e-6 && g[2].abs() < 1e-6 && g[3] == 0.0 && g[5].abs() < 1e-6);
        assert!((power(&g) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn vbap_power_sum_is_one_everywhere() {
        for layout in ChannelLayout::ALL {
            let p = VbapPanner::new(layout);
            let mut az = -180.0f32;
            while az <= 180.0 {
                for el in [-30.0f32, 0.0, 20.0, 45.0, 80.0] {
                    let g = p.gains(az, el);
                    assert!(
                        (power(&g) - 1.0).abs() < 1e-3,
                        "{} az {az} el {el}: {g:?}",
                        layout.label()
                    );
                    assert!(g.iter().all(|x| *x >= 0.0));
                    if let Some(l) = layout.lfe_index() {
                        assert_eq!(g[l], 0.0);
                    }
                }
                az += 15.0;
            }
        }
    }

    #[test]
    fn vbap_3d_uses_height_speakers() {
        let p = VbapPanner::new(ChannelLayout::Atmos714);
        assert!(p.base_count() >= 10);
        let g = p.gains(45.0, 45.0);
        assert!((g[8] - 1.0).abs() < 1e-3, "{g:?}");
        // Straight up: only the four height speakers contribute.
        let g = p.gains(0.0, 90.0);
        let bed: f32 = g[..8].iter().sum();
        let top: f32 = g[8..].iter().sum();
        assert!(bed < 1e-3 && top > 0.9, "{g:?}");
        // Between front-left bed and its height speaker: only those two.
        let g = p.gains(30.0, 22.0);
        assert!(g[0] > 0.1 && g[8] > 0.1, "{g:?}");
    }

    #[test]
    fn spread_and_objects() {
        let p = VbapPanner::new(ChannelLayout::Surround51);
        let point = p.gains(0.0, 0.0);
        let wide = p.gains_with_spread(0.0, 0.0, 120.0);
        assert!((power(&wide) - 1.0).abs() < 1e-3);
        assert!(
            wide[0] > point[0] && wide[1] > point[1],
            "spread should leak into L/R: {wide:?}"
        );
        let omni = p.gains_with_spread(0.0, 0.0, 360.0);
        assert!(omni[4] > 0.05 && omni[5] > 0.05, "{omni:?}");

        let params = ObjectPanParams {
            lfe_send_db: Some(-10.0),
            ..Default::default()
        };
        let near = p.pan_object([1.0, 0.0, 0.0], &params); // 1 m in front
        assert!((near[2] - 1.0).abs() < 1e-3, "{near:?}");
        assert!((near[3] - db_to_lin(-10.0)).abs() < 1e-4);
        let far = p.pan_object([4.0, 0.0, 0.0], &params);
        assert!((far[2] - 0.25).abs() < 1e-3, "{far:?}");
        let left = p.pan_object([0.0, 2.0, 0.0], &ObjectPanParams::default());
        // 90° left sits between L (30) and Ls (110).
        assert!(left[0] > 0.0 && left[4] > 0.0 && left[1] == 0.0 && left[3] == 0.0);
        let at_listener = p.pan_object([0.0, 0.0, 0.0], &ObjectPanParams::default());
        assert!(at_listener
            .iter()
            .enumerate()
            .all(|(i, g)| i == 3 || *g > 0.0));
    }

    #[test]
    fn downmix_matrices() {
        // 5.1 with centre only → both stereo channels at −3 dB.
        let mut b = AudioBuffer::new(48_000, 6, 4);
        for f in 0..4 {
            *b.sample_mut(f, 2) = 1.0;
            *b.sample_mut(f, 3) = 0.7; // LFE is dropped
        }
        let st = downmix(&b, ChannelLayout::Surround51, ChannelLayout::Stereo).unwrap();
        assert_eq!(st.channels, 2);
        assert!((st.sample(0, 0) - M3).abs() < 1e-6 && (st.sample(0, 1) - M3).abs() < 1e-6);
        // Ls goes to L only.
        let mut b = AudioBuffer::new(48_000, 6, 1);
        *b.sample_mut(0, 4) = 1.0;
        let st = downmix(&b, ChannelLayout::Surround51, ChannelLayout::Stereo).unwrap();
        assert!((st.sample(0, 0) - M3).abs() < 1e-6 && st.sample(0, 1).abs() < 1e-6);
        // 7.1 → 5.1: sides and rears fold into Ls/Rs.
        let mut b = AudioBuffer::new(48_000, 8, 1);
        *b.sample_mut(0, 4) = 1.0;
        *b.sample_mut(0, 6) = 1.0;
        let s51 = downmix(&b, ChannelLayout::Surround71, ChannelLayout::Surround51).unwrap();
        assert_eq!(s51.channels, 6);
        assert!((s51.sample(0, 4) - 2.0 * M3).abs() < 1e-5 && s51.sample(0, 5).abs() < 1e-6);
        // Composed path 7.1.4 → stereo works and keeps the centre.
        let mut b = AudioBuffer::new(48_000, 12, 1);
        *b.sample_mut(0, 2) = 1.0;
        let st = downmix(&b, ChannelLayout::Atmos714, ChannelLayout::Stereo).unwrap();
        assert!((st.sample(0, 0) - M3).abs() < 1e-5);
        assert!(Downmix::between(ChannelLayout::Stereo, ChannelLayout::Atmos714).is_some());
        assert!(downmix(
            &AudioBuffer::new(48_000, 2, 1),
            ChannelLayout::Surround51,
            ChannelLayout::Stereo
        )
        .is_err());
    }
}
