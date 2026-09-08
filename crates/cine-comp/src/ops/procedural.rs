//! Procedural pattern generation: fBm Perlin noise, gradients, checkers,
//! radial falloffs and Worley (cellular) noise.

use crate::graph_model::ProceduralKind;
use crate::image::Image;
use rayon::prelude::*;

/// Parameters for [`generate`].
#[derive(Clone, Debug, PartialEq)]
pub struct Procedural {
    pub kind: ProceduralKind,
    pub width: usize,
    pub height: usize,
    pub seed: u64,
    pub frequency: f32,
    pub octaves: usize,
    pub lacunarity: f32,
    pub gain: f32,
    pub color1: [f32; 4],
    pub color2: [f32; 4],
    pub angle_deg: f32,
    pub center: [f32; 2],
    pub radius: f32,
    pub softness: f32,
    pub size: usize,
}

impl Default for Procedural {
    fn default() -> Self {
        Procedural {
            kind: ProceduralKind::Noise,
            width: 256,
            height: 256,
            seed: 0,
            frequency: 4.0,
            octaves: 4,
            lacunarity: 2.0,
            gain: 0.5,
            color1: [0.0, 0.0, 0.0, 1.0],
            color2: [1.0, 1.0, 1.0, 1.0],
            angle_deg: 0.0,
            center: [0.5, 0.5],
            radius: 0.5,
            softness: 0.5,
            size: 16,
        }
    }
}

/// Seeded permutation table for Perlin noise.
#[derive(Clone, Debug)]
pub struct Perlin {
    perm: [u8; 512],
}

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Perlin {
    /// Table shuffled with `seed`.
    pub fn new(seed: u64) -> Self {
        let mut p: Vec<u8> = (0..=255).collect();
        let mut s = seed ^ 0xA5A5_5A5A_1234_5678;
        for i in (1..256).rev() {
            let j = (splitmix(&mut s) % (i as u64 + 1)) as usize;
            p.swap(i, j);
        }
        let mut perm = [0u8; 512];
        for (i, v) in perm.iter_mut().enumerate() {
            *v = p[i & 255];
        }
        Perlin { perm }
    }

    fn grad(hash: u8, x: f32, y: f32) -> f32 {
        match hash & 7 {
            0 => x + y,
            1 => -x + y,
            2 => x - y,
            3 => -x - y,
            4 => x,
            5 => -x,
            6 => y,
            _ => -y,
        }
    }

    /// 2D Perlin noise in roughly `[-1, 1]`.
    pub fn noise(&self, x: f32, y: f32) -> f32 {
        let xi = x.floor();
        let yi = y.floor();
        let xf = x - xi;
        let yf = y - yi;
        let xi = (xi as i64 & 255) as usize;
        let yi = (yi as i64 & 255) as usize;
        let fade = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
        let u = fade(xf);
        let v = fade(yf);
        let p = &self.perm;
        let aa = p[p[xi] as usize + yi];
        let ab = p[p[xi] as usize + yi + 1];
        let ba = p[p[xi + 1] as usize + yi];
        let bb = p[p[xi + 1] as usize + yi + 1];
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let x1 = lerp(Self::grad(aa, xf, yf), Self::grad(ba, xf - 1.0, yf), u);
        let x2 = lerp(
            Self::grad(ab, xf, yf - 1.0),
            Self::grad(bb, xf - 1.0, yf - 1.0),
            u,
        );
        lerp(x1, x2, v)
    }

    /// Fractional Brownian motion, normalized to roughly `[0, 1]`.
    pub fn fbm(&self, x: f32, y: f32, octaves: usize, lacunarity: f32, gain: f32) -> f32 {
        let mut amp = 1.0;
        let mut freq = 1.0;
        let mut sum = 0.0;
        let mut norm = 0.0;
        for _ in 0..octaves.max(1) {
            sum += amp * self.noise(x * freq, y * freq);
            norm += amp;
            amp *= gain;
            freq *= lacunarity;
        }
        (sum / norm.max(1e-6)) * 0.5 + 0.5
    }
}

/// Worley (cellular) noise: distance to the nearest feature point (one per
/// unit cell), normalized so typical values fall in `[0, 1]`.
pub fn worley(x: f32, y: f32, seed: u64) -> f32 {
    let xi = x.floor() as i64;
    let yi = y.floor() as i64;
    let mut best = f32::INFINITY;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let cx = xi + dx;
            let cy = yi + dy;
            let mut s = seed
                ^ ((cx as u64).wrapping_mul(0x9E37_79B9))
                ^ ((cy as u64).wrapping_mul(0x85EB_CA6B) << 16);
            let fx = (splitmix(&mut s) % 10_000) as f32 / 10_000.0;
            let fy = (splitmix(&mut s) % 10_000) as f32 / 10_000.0;
            let px = cx as f32 + fx;
            let py = cy as f32 + fy;
            let d = ((px - x).powi(2) + (py - y).powi(2)).sqrt();
            best = best.min(d);
        }
    }
    best.min(1.0)
}

fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let mut o = [0.0; 4];
    for i in 0..4 {
        o[i] = a[i] + (b[i] - a[i]) * t;
    }
    o
}

/// Pattern value in `[0, 1]` at normalized coordinates `(u, v)` / pixel `(x, y)`.
pub fn pattern_value(p: &Procedural, perlin: &Perlin, x: usize, y: usize) -> f32 {
    let u = (x as f32 + 0.5) / p.width.max(1) as f32;
    let v = (y as f32 + 0.5) / p.height.max(1) as f32;
    match p.kind {
        ProceduralKind::Noise => perlin.fbm(
            u * p.frequency,
            v * p.frequency,
            p.octaves,
            p.lacunarity,
            p.gain,
        ),
        ProceduralKind::Gradient => {
            let a = p.angle_deg.to_radians();
            let (dx, dy) = (a.cos(), a.sin());
            let proj = (u - 0.5) * dx + (v - 0.5) * dy;
            let extent = (dx.abs() + dy.abs()) * 0.5;
            if extent > 0.0 {
                (proj / extent * 0.5 + 0.5).clamp(0.0, 1.0)
            } else {
                0.5
            }
        }
        ProceduralKind::Checker => {
            let s = p.size.max(1);
            if ((x / s) + (y / s)).is_multiple_of(2) {
                0.0
            } else {
                1.0
            }
        }
        ProceduralKind::Radial => {
            let d = ((u - p.center[0]).powi(2) + (v - p.center[1]).powi(2)).sqrt();
            let r = p.radius.max(1e-6);
            let soft = p.softness.clamp(0.0, 1.0) * r;
            if soft <= 0.0 {
                if d <= r {
                    1.0
                } else {
                    0.0
                }
            } else {
                (1.0 - (d - (r - soft)) / soft).clamp(0.0, 1.0)
            }
        }
        ProceduralKind::Cellular => worley(u * p.frequency, v * p.frequency, p.seed),
    }
}

/// Generate the pattern as an RGBA image blending `color1` (0) to `color2` (1).
pub fn generate(p: &Procedural) -> Image {
    let perlin = Perlin::new(p.seed);
    let mut out = Image::rgba(p.width, p.height);
    if out.is_empty() {
        return out;
    }
    let w = p.width;
    out.data
        .par_chunks_exact_mut(4 * w)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let t = pattern_value(p, &perlin, x, y);
                px.copy_from_slice(&lerp4(p.color1, p.color2, t));
            }
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_deterministic_and_bounded() {
        let a = generate(&Procedural {
            width: 32,
            height: 32,
            seed: 7,
            ..Default::default()
        });
        let b = generate(&Procedural {
            width: 32,
            height: 32,
            seed: 7,
            ..Default::default()
        });
        let c = generate(&Procedural {
            width: 32,
            height: 32,
            seed: 8,
            ..Default::default()
        });
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.data.iter().all(|v| (-0.01..=1.01).contains(v)));
        let mean = a.channel_mean(0);
        assert!(mean > 0.2 && mean < 0.8, "mean {mean}");
    }

    #[test]
    fn simple_patterns() {
        let g = generate(&Procedural {
            kind: ProceduralKind::Gradient,
            width: 16,
            height: 1,
            ..Default::default()
        });
        assert!(g.get_rgba(0, 0)[0] < g.get_rgba(15, 0)[0]);
        let ch = generate(&Procedural {
            kind: ProceduralKind::Checker,
            width: 4,
            height: 4,
            size: 2,
            ..Default::default()
        });
        assert_eq!(ch.get_rgba(0, 0)[0], 0.0);
        assert_eq!(ch.get_rgba(2, 0)[0], 1.0);
        let r = generate(&Procedural {
            kind: ProceduralKind::Radial,
            width: 32,
            height: 32,
            softness: 0.0,
            ..Default::default()
        });
        assert_eq!(r.get_rgba(16, 16)[0], 1.0);
        assert_eq!(r.get_rgba(0, 0)[0], 0.0);
        let c = generate(&Procedural {
            kind: ProceduralKind::Cellular,
            width: 16,
            height: 16,
            ..Default::default()
        });
        assert!(c.data.iter().all(|v| (0.0..=1.0).contains(v)));
    }
}
