//! A stable 64-bit hash (FNV-1a). Values end up in the committed baseline and
//! on-disk caches, so they must not change between Rust releases.

#[derive(Clone, Copy, Debug)]
pub struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    pub fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    pub fn write_u64(&mut self, v: u64) {
        self.write(&v.to_le_bytes());
    }

    pub fn finish(self) -> u64 {
        self.0
    }
}

pub fn fnv64(bytes: &[u8]) -> u64 {
    let mut h = Fnv::default();
    h.write(bytes);
    h.finish()
}

#[cfg(test)]
mod tests {
    #[test]
    fn known_vector() {
        assert_eq!(super::fnv64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
