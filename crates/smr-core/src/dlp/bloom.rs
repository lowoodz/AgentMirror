//! Simple Bloom filter for signature pre-filtering (mmap-friendly bit vector).

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

const DEFAULT_K: u32 = 7;

#[derive(Clone)]
pub struct BloomFilter {
    words: Vec<u64>,
    bit_count: usize,
    k: u32,
}

impl BloomFilter {
    pub fn new(bit_count: usize) -> Self {
        let words_len = bit_count.div_ceil(64).max(1);
        Self {
            words: vec![0u64; words_len],
            bit_count,
            k: DEFAULT_K,
        }
    }

    pub fn insert_hash(&mut self, hash: u64) {
        for bit in hash_positions(hash, self.bit_count, self.k) {
            self.words[bit / 64] |= 1u64 << (bit % 64);
        }
    }

    pub fn may_contain(&self, hash: u64) -> bool {
        hash_positions(hash, self.bit_count, self.k).all(|bit| {
            (self.words[bit / 64] & (1u64 << (bit % 64))) != 0
        })
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut f = File::create(path)?;
        f.write_all(&(self.bit_count as u64).to_le_bytes())?;
        f.write_all(&self.k.to_le_bytes())?;
        for w in &self.words {
            f.write_all(&w.to_le_bytes())?;
        }
        Ok(())
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        let mut f = File::open(path)?;
        let mut buf8 = [0u8; 8];
        f.read_exact(&mut buf8)?;
        let bit_count = u64::from_le_bytes(buf8) as usize;
        let mut buf4 = [0u8; 4];
        f.read_exact(&mut buf4)?;
        let k = u32::from_le_bytes(buf4);
        let words_len = bit_count.div_ceil(64).max(1);
        let mut words = Vec::with_capacity(words_len);
        for _ in 0..words_len {
            f.read_exact(&mut buf8)?;
            words.push(u64::from_le_bytes(buf8));
        }
        Ok(Self {
            words,
            bit_count,
            k,
        })
    }
}

fn hash_positions(hash: u64, bit_count: usize, k: u32) -> impl Iterator<Item = usize> {
    let h1 = hash;
    let h2 = hash.rotate_left(17) | 1;
    (0..k).map(move |i| {
        let combined = h1.wrapping_add((i as u64).wrapping_mul(h2));
        (combined as usize) % bit_count
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bloom_insert_contains() {
        let mut b = BloomFilter::new(1 << 16);
        b.insert_hash(0xDEADBEEF);
        assert!(b.may_contain(0xDEADBEEF));
        assert!(!b.may_contain(0xCAFEBABE));
    }
}
