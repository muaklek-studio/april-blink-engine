//! FNV-1a hashing: tiny, deterministic across platforms and builds. Used for pack path hashes.
//! Not suitable for untrusted input in hash maps (no DoS resistance).

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x100_0000_01b3;

pub const fn fnv1a64(data: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    let mut i = 0;
    while i < data.len() {
        hash ^= data[i] as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
        i += 1;
    }
    hash
}

pub const fn fnv1a64_str(s: &str) -> u64 {
    fnv1a64(s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_reference_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64_str("foobar"), 0x8594_4171_f739_67e8);
    }
}
