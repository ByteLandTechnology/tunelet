//! A deterministic 48-bit linear congruential stream for random audio
//! variation.
//!
//! Each processor has one stream. Choir drift and aspiration use the same
//! stream. The order in which stages read values is part of the repeatable
//! result. If you enable a stage that reads the stream, later stages get
//! different values. This generator is for audio. It is not a cryptographic
//! random source. An audio reset does not set a new seed.

/// The 48-bit linear congruential state, made from a 32-bit seed.
#[derive(Debug)]
pub(crate) struct RandomStream {
    /// The low 48 bits of the recurrence. Each step clears the higher bits.
    state: u64,
}
impl RandomStream {
    /// Creates a stream with the state `(seed << 16) | 0x330e`. The sequence
    /// does not advance.
    pub(crate) fn new(seed: u32) -> Self {
        Self {
            state: ((seed as u64) << 16) | 0x330e,
        }
    }
    /// Advances the state and returns the high 31 bits as a nonnegative
    /// `i32`.
    ///
    /// The step uses the multiplier `0x5deece66d` and the increment 11,
    /// modulo 2^48.
    pub(crate) fn next_u31(&mut self) -> i32 {
        self.state = self.state.wrapping_mul(0x5deece66d).wrapping_add(11) & ((1_u64 << 48) - 1);
        (self.state >> 17) as i32
    }
}
