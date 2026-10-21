//! Stable state hashing for determinism checks.
//!
//! `std`'s `DefaultHasher` is not guaranteed stable across Rust releases, so
//! golden hashes would break on toolchain upgrades. FNV-1a is tiny, fast
//! enough for a once-per-check hash, and stable forever.

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// A stable, platform-independent 64-bit hasher (FNV-1a).
///
/// Modules contribute their state with the `write_*` methods; all integers
/// are fed in little-endian order so results match on every target.
#[derive(Debug, Clone)]
pub struct StateHasher(u64);

impl Default for StateHasher {
    fn default() -> Self {
        Self(FNV_OFFSET)
    }
}

impl StateHasher {
    /// Creates a hasher in its initial state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds raw bytes.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }

    /// Feeds a `u64`.
    pub fn write_u64(&mut self, value: u64) {
        self.write_bytes(&value.to_le_bytes());
    }

    /// Feeds a `u128`.
    pub fn write_u128(&mut self, value: u128) {
        self.write_bytes(&value.to_le_bytes());
    }

    /// Feeds an `f64` by its exact bit pattern.
    pub fn write_f64(&mut self, value: f64) {
        self.write_u64(value.to_bits());
    }

    /// Returns the hash of everything written so far.
    #[must_use]
    pub fn finish(&self) -> u64 {
        self.0
    }
}
