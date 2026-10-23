//! Seeded gradient noise for terrain and natural variation.

use super::Rng;

/// 2D gradient (Perlin) noise from a seed: smooth values in about `-1..1`.
#[derive(Debug, Clone)]
pub struct Noise {
    perm: [u8; 512],
}

impl Noise {
    pub fn new(seed: u64) -> Self {
        let mut table: Vec<u8> = (0..=255).collect();
        Rng::new(seed).shuffle(&mut table);
        Self {
            perm: std::array::from_fn(|i| table[i & 255]),
        }
    }

    fn gradient(&self, ix: i32, iy: i32, x: f32, y: f32) -> f32 {
        let h = self.perm[(self.perm[(ix & 255) as usize] as usize + (iy & 255) as usize) & 511];
        let (gx, gy) = match h & 7 {
            0 => (1.0, 0.0),
            1 => (-1.0, 0.0),
            2 => (0.0, 1.0),
            3 => (0.0, -1.0),
            4 => (0.7071, 0.7071),
            5 => (-0.7071, 0.7071),
            6 => (0.7071, -0.7071),
            _ => (-0.7071, -0.7071),
        };
        gx * x + gy * y
    }

    /// Noise at a point.
    pub fn get(&self, x: f32, y: f32) -> f32 {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (ix, iy) = (x0 as i32, y0 as i32);
        let fade = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
        let (u, v) = (fade(fx), fade(fy));
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let a = self.gradient(ix, iy, fx, fy);
        let b = self.gradient(ix + 1, iy, fx - 1.0, fy);
        let c = self.gradient(ix, iy + 1, fx, fy - 1.0);
        let d = self.gradient(ix + 1, iy + 1, fx - 1.0, fy - 1.0);
        lerp(lerp(a, b, u), lerp(c, d, u), v) * 1.4
    }

    /// Fractal noise: `octaves` layers, each `lacunarity` times finer and `gain` times
    /// weaker. Normalised to about `-1..1`.
    pub fn fbm(&self, x: f32, y: f32, octaves: u32, lacunarity: f32, gain: f32) -> f32 {
        let (mut sum, mut amplitude, mut frequency, mut total) = (0.0, 1.0, 1.0, 0.0);
        for _ in 0..octaves.max(1) {
            sum += self.get(x * frequency, y * frequency) * amplitude;
            total += amplitude;
            amplitude *= gain;
            frequency *= lacunarity;
        }
        sum / total
    }

    /// Ridged fractal noise (sharp mountain crests): `0..1`.
    pub fn ridged(&self, x: f32, y: f32, octaves: u32) -> f32 {
        let (mut sum, mut amplitude, mut frequency, mut total) = (0.0, 1.0, 1.0, 0.0);
        for _ in 0..octaves.max(1) {
            let n = 1.0 - self.get(x * frequency, y * frequency).abs();
            sum += n * n * amplitude;
            total += amplitude;
            amplitude *= 0.5;
            frequency *= 2.0;
        }
        sum / total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_smooth_bounded_and_seeded() {
        let n = Noise::new(9);
        let mut max: f32 = 0.0;
        for i in 0..2000 {
            let (x, y) = (i as f32 * 0.137, i as f32 * 0.071);
            let v = n.fbm(x, y, 5, 2.0, 0.5);
            max = max.max(v.abs());
            // Smooth: a tiny step changes it a little.
            assert!((n.get(x, y) - n.get(x + 0.001, y)).abs() < 0.01);
            let r = n.ridged(x, y, 4);
            assert!((0.0..=1.0).contains(&r));
        }
        assert!(max <= 1.2 && max > 0.2, "{max}");
        assert_eq!(Noise::new(9).get(3.3, 1.7), n.get(3.3, 1.7));
        assert_ne!(Noise::new(10).get(3.3, 1.7), n.get(3.3, 1.7));
        // Integer points are zero (gradient noise).
        assert_eq!(n.get(2.0, 5.0), 0.0);
    }
}
