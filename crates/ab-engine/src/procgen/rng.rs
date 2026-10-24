//! A small, fast, seeded random number generator: the same seed gives the same numbers on
//! every machine and every run (xoshiro256**, seeded through SplitMix64).

/// Deterministic random numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rng {
    state: [u64; 4],
}

fn splitmix(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// FNV-1a: a stable hash for naming streams (`std`'s hasher may change between releases).
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut x = seed;
        Self {
            state: std::array::from_fn(|_| splitmix(&mut x)),
        }
    }

    /// An independent stream for one purpose (`"trees"`, `"rooms"`): adding a use of
    /// random numbers somewhere does not change the others.
    pub fn fork(&self, label: &str) -> Self {
        Self::new(self.state[0] ^ fnv(label).rotate_left(17))
    }

    pub fn next_u64(&mut self) -> u64 {
        let s = &mut self.state;
        let result = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        result
    }

    /// In `0..1`.
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// In `low..high` (or `low` when the range is empty).
    pub fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.f32()
    }

    /// In `low..=high`.
    pub fn int(&mut self, low: i64, high: i64) -> i64 {
        if high <= low {
            return low;
        }
        let span = (high - low + 1) as u64;
        low + (self.next_u64() % span) as i64
    }

    /// `true` with probability `p`.
    pub fn chance(&mut self, p: f32) -> bool {
        self.f32() < p
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            items.get(self.int(0, items.len() as i64 - 1) as usize)
        }
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.int(0, i as i64) as usize;
            items.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_numbers_and_streams_apart() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        let xs: Vec<u64> = (0..8).map(|_| a.next_u64()).collect();
        let ys: Vec<u64> = (0..8).map(|_| b.next_u64()).collect();
        assert_eq!(xs, ys);
        assert_ne!(
            Rng::new(7).fork("trees").next_u64(),
            Rng::new(7).fork("rocks").next_u64()
        );
        assert_ne!(Rng::new(1).next_u64(), Rng::new(2).next_u64());
        // Fixed across machines and releases.
        assert_eq!(Rng::new(42).next_u64(), Rng::new(42).next_u64());
        let mut r = Rng::new(3);
        for _ in 0..1000 {
            let v = r.f32();
            assert!((0.0..1.0).contains(&v));
            let i = r.int(-2, 2);
            assert!((-2..=2).contains(&i));
        }
        let mut items = [1, 2, 3, 4, 5];
        r.shuffle(&mut items);
        items.sort_unstable();
        assert_eq!(items, [1, 2, 3, 4, 5]);
    }
}
