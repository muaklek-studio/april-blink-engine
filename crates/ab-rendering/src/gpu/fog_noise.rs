//! The noise volume that breaks fog into drifting patches (see `fog.wgsl`).
//!
//! A small tiling 3D texture of smooth value noise, made once on the CPU: the fog shader
//! samples it twice per pixel with trilinear filtering, which costs far less than computing
//! noise per pixel or marching through the fog.

/// Texels along each side of the volume.
pub(crate) const SIZE: u32 = 32;

/// Smooth, tiling fractal noise stretched to the full `0..=255`, `SIZE`³ texels, x fastest.
pub(crate) fn volume() -> Vec<u8> {
    let n = SIZE as usize;
    let mut values = Vec::with_capacity(n * n * n);
    // Octaves: (lattice cells across the volume, weight). Whole cell counts keep it tiling.
    const OCTAVES: [(u32, f32); 3] = [(4, 0.6), (8, 0.28), (16, 0.12)];
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let p = [x, y, z].map(|c| c as f32 / n as f32);
                let value: f32 = OCTAVES
                    .iter()
                    .enumerate()
                    .map(|(i, &(cells, weight))| weight * value_noise(p, cells, i as u32))
                    .sum();
                values.push(value);
            }
        }
    }
    // Averaging octaves narrows the range: stretch it back so patches have contrast.
    let (min, max) = values
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    let span = (max - min).max(1e-6);
    values
        .into_iter()
        .map(|v| ((v - min) / span * 255.0).round() as u8)
        .collect()
}

/// Value noise in `0..=1` at `p` (`0..1` per axis) on a lattice of `cells` cells per axis
/// that wraps around; `seed` decorrelates octaves.
fn value_noise(p: [f32; 3], cells: u32, seed: u32) -> f32 {
    let scaled = p.map(|c| c * cells as f32);
    let cell = scaled.map(|c| c.floor());
    let t = [0, 1, 2].map(|i| fade(scaled[i] - cell[i]));
    let corner = |dx: u32, dy: u32, dz: u32| {
        let wrap = |c: f32, d: u32| (c as u32 + d) % cells;
        lattice(
            wrap(cell[0], dx),
            wrap(cell[1], dy),
            wrap(cell[2], dz),
            seed,
        )
    };
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = lerp(corner(0, 0, 0), corner(1, 0, 0), t[0]);
    let x10 = lerp(corner(0, 1, 0), corner(1, 1, 0), t[0]);
    let x01 = lerp(corner(0, 0, 1), corner(1, 0, 1), t[0]);
    let x11 = lerp(corner(0, 1, 1), corner(1, 1, 1), t[0]);
    lerp(lerp(x00, x10, t[1]), lerp(x01, x11, t[1]), t[2])
}

/// Quintic ease: smooth value and slope at the lattice points.
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// A pseudo-random value in `0..=1` for a lattice point.
fn lattice(x: u32, y: u32, z: u32, seed: u32) -> f32 {
    let mut h = x
        .wrapping_mul(0x8da6_b343)
        .wrapping_add(y.wrapping_mul(0xd816_3841))
        .wrapping_add(z.wrapping_mul(0xcb1a_b31f))
        .wrapping_add(seed.wrapping_mul(0x9e37_79b9));
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

/// Uploads the volume as an `R8Unorm` 3D texture, sampled with a repeating linear sampler.
pub(crate) fn create(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::TextureView, wgpu::Sampler) {
    let size = wgpu::Extent3d {
        width: SIZE,
        height: SIZE,
        depth_or_array_layers: SIZE,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fog noise"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        &volume(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(SIZE),
            rows_per_image: Some(SIZE),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("fog noise sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    (view, sampler)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(volume: &[u8], x: u32, y: u32, z: u32) -> u8 {
        let n = SIZE;
        volume[((z % n) * n * n + (y % n) * n + (x % n)) as usize]
    }

    #[test]
    fn spans_a_range_around_the_middle() {
        let volume = volume();
        assert_eq!(volume.len(), (SIZE * SIZE * SIZE) as usize);
        let mean = volume.iter().map(|&v| f32::from(v)).sum::<f32>() / volume.len() as f32;
        assert!((90.0..165.0).contains(&mean), "mean {mean}");
        let (min, max) = (volume.iter().min().unwrap(), volume.iter().max().unwrap());
        assert_eq!((*min, *max), (0, 255), "the full range");
    }

    #[test]
    fn is_smooth_and_tiles() {
        let volume = volume();
        let step = |a: u8, b: u8| (i32::from(a) - i32::from(b)).abs();
        // Largest change between neighbours inside the volume and across its wrap-around seam.
        let (mut inside, mut seam) = (0, 0);
        for z in 0..SIZE {
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let here = at(&volume, x, y, z);
                    for (next, wraps) in [
                        (at(&volume, x + 1, y, z), x + 1 == SIZE),
                        (at(&volume, x, y + 1, z), y + 1 == SIZE),
                        (at(&volume, x, y, z + 1), z + 1 == SIZE),
                    ] {
                        let d = step(here, next);
                        if wraps {
                            seam = seam.max(d);
                        } else {
                            inside = inside.max(d);
                        }
                    }
                }
            }
        }
        assert!(inside < 64, "largest step inside: {inside}");
        assert!(
            seam <= inside,
            "the seam ({seam}) is rougher than the inside ({inside})"
        );
    }
}
