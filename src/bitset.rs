use smallvec::SmallVec;

/// Compact bitset backed by a `SmallVec<[u64; 4]>`.
///
/// Bit layout: bits 0–63 in `data[0]`, bits 64–127 in `data[1]`, etc.
/// Mirrors `bitset.go` from the original porcupine implementation.
///
/// Inline capacity of 4 covers up to 256 operations without heap allocation.
/// Typical histories (etcd ~170 ops → 3 chunks, KV per-partition ≤50 ops → 1 chunk)
/// fit entirely on the stack, eliminating heap allocation on every `clone()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitset(SmallVec<[u64; 4]>);

impl Bitset {
    /// Allocate a bitset large enough to hold `n` bits, all initially zero.
    pub fn new(n: usize) -> Self {
        let chunks = n.div_ceil(64);
        let mut data: SmallVec<[u64; 4]> = SmallVec::new();
        data.resize(chunks, 0u64);
        Bitset(data)
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
