//! Seeded, deterministic RNG standing in for `UnityEngine.Random`.
//! Sequences won't match the original; only the distributions do.

/// xorshift32.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Uniform in `[0, 1)`. orig: `Random.value`.
    pub fn value(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Uniform in `[min, max)`. orig: `Random.Range(float, float)`.
    pub fn range(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.value()
    }

    /// Uniform integer in `[min, max)`; returns `min` when `max <= min`. orig: `Random.Range(int, int)`.
    pub fn range_i32(&mut self, min: i32, max: i32) -> i32 {
        if max <= min {
            return min;
        }
        let n = (max as i64 - min as i64) as u64;
        // xorshift32 visits every nonzero u32: sample that domain evenly.
        let limit = u32::MAX as u64 / n * n;
        loop {
            let v = self.next_u32() as u64 - 1;
            if v < limit {
                return (min as i64 + (v % n) as i64) as i32;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_in_range() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 {
            let v = a.value();
            assert_eq!(v, b.value());
            assert!((0.0..1.0).contains(&v));
            let r = a.range(-0.5, 0.5);
            b.range(-0.5, 0.5);
            assert!((-0.5..0.5).contains(&r));
        }
    }
}
