//! Helpers shared by the ported test suites.
#![allow(dead_code)]

/// A small deterministic random number generator (xorshift64*), so the
/// randomized tests from CodeMirror are reproducible.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A number in `0..n` (0 when `n` is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }

    pub fn string(&mut self, len: usize) -> String {
        (0..len).map(|_| (b'a' + self.below(26) as u8) as char).collect()
    }
}
