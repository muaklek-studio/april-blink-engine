//! Minimal UUID v4 generator (replacement path for the `uuid` crate).
//!
//! Randomness comes from SplitMix64 seeded by the clock, a process-wide counter and a stack
//! address, which is plenty for asset/scene identifiers. It is **not** cryptographically secure.

use super::rand::SimpleRng;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Uuid([u8; 16]);

impl Uuid {
    pub const NIL: Self = Self([0; 16]);

    pub fn new_v4() -> Self {
        let mut rng = SimpleRng::new(entropy_seed());
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&rng.next_u64().to_le_bytes());
        bytes[8..].copy_from_slice(&rng.next_u64().to_le_bytes());
        bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
        bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant
        Self(bytes)
    }

    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl Default for Uuid {
    fn default() -> Self {
        Self::new_v4()
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, b) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// Unique-per-call seed: clock nanos, a monotonically increasing counter and a stack address.
pub(crate) fn entropy_seed() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let local = 0u8;
    let addr = &local as *const u8 as u64;
    nanos ^ count.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ addr.rotate_left(32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn sets_version_and_variant_bits() {
        let id = Uuid::new_v4();
        assert_eq!(id.as_bytes()[6] >> 4, 4);
        assert_eq!(id.as_bytes()[8] >> 6, 0b10);
    }

    #[test]
    fn consecutive_ids_are_unique() {
        let ids: HashSet<_> = (0..10_000).map(|_| Uuid::new_v4()).collect();
        assert_eq!(ids.len(), 10_000);
    }

    #[test]
    fn display_is_canonical() {
        let s = Uuid::from_bytes([0xab; 16]).to_string();
        assert_eq!(s, "abababab-abab-abab-abab-abababababab");
    }
}
