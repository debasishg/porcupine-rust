// Production backing: SmallVec<[u64; 4]> gives zero heap allocation for ≤256 ops.
// Verify backing:     Vec<u64> — no external-crate dependency for Charon extraction.
#[cfg(not(feature = "verify"))]
use smallvec::SmallVec;

/// Compact bitset for tracking linearized operations and as a DFS cache key.
///
/// Bit layout: bits 0–63 in chunk 0, bits 64–127 in chunk 1, etc.
/// Mirrors `bitset.go` from the original porcupine implementation.
///
/// **Production** (`SmallVec<[u64; 4]>`): inline capacity covers up to 256 ops
/// without heap allocation; typical etcd (~170 ops) and KV (≤50 ops/partition)
/// histories fit entirely on the stack.
///
/// **Verify** (`Vec<u64>`): same semantics, no external-crate dependency,
/// enabling clean Charon/Aeneas LLBC extraction (`--features verify`).
#[cfg(not(feature = "verify"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitset(SmallVec<[u64; 4]>);

#[cfg(feature = "verify")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitset(Vec<u64>);

impl Bitset {
    /// Allocate a bitset large enough to hold `n` bits, all initially zero.
    pub fn new(n: usize) -> Self {
        let chunks = n.div_ceil(64);
        #[cfg(not(feature = "verify"))]
        {
            let mut data: SmallVec<[u64; 4]> = SmallVec::new();
            data.resize(chunks, 0u64);
            Bitset(data)
        }
        #[cfg(feature = "verify")]
        {
            Bitset(vec![0u64; chunks])
        }
    }

    fn index(pos: usize) -> (usize, usize) {
        (pos / 64, pos % 64)
    }

    /// Set bit at `pos`.
    pub fn set(&mut self, pos: usize) {
        let (major, minor) = Self::index(pos);
        self.0[major] |= 1u64 << minor;
    }

    /// Clear bit at `pos`.
    pub fn clear(&mut self, pos: usize) {
        let (major, minor) = Self::index(pos);
        self.0[major] &= !(1u64 << minor);
    }

    /// Count the number of set bits.
    pub fn popcnt(&self) -> usize {
        self.0.iter().map(|v| v.count_ones() as usize).sum()
    }

    /// Hash of the bitset. Matches the Go implementation:
    /// `hash = popcnt; for each chunk: hash ^= chunk`.
    pub fn hash(&self) -> u64 {
        let mut h = self.popcnt() as u64;
        for &v in &self.0 {
            h ^= v;
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_clear() {
        let mut b = Bitset::new(128);
        b.set(0);
        b.set(63);
        b.set(64);
        b.set(127);
        assert_eq!(b.popcnt(), 4);
        b.clear(63);
        assert_eq!(b.popcnt(), 3);
    }

    #[test]
    fn test_hash_deterministic() {
        let mut b1 = Bitset::new(64);
        let mut b2 = Bitset::new(64);
        b1.set(3);
        b1.set(7);
        b2.set(3);
        b2.set(7);
        assert_eq!(b1.hash(), b2.hash());
        assert_eq!(b1, b2);
    }

    #[test]
    fn test_clone_independence() {
        let mut b1 = Bitset::new(64);
        b1.set(5);
        let mut b2 = b1.clone();
        b2.set(10);
        assert_eq!(b1.popcnt(), 1);
        assert_eq!(b2.popcnt(), 2);
    }
}
