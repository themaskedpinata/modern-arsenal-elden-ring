//! xorshift64* random numbers: deterministic in tests, seeded from the clock + pid in the DLL.

/// xorshift64* generator (Vigna 2016). Not cryptographic; used only for spread cones.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A zero seed is replaced by a fixed odd constant (xorshift must never hold 0).
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_and_determinism() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..10_000 {
            let x = a.next_f32();
            assert!((0.0..1.0).contains(&x));
            assert_eq!(x, b.next_f32());
        }
        let mut z = Rng::new(0);
        assert_ne!(z.next_u64(), 0);
    }
}
