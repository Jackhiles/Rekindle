//! A fast, non-cryptographic hash for the runtime's internal maps (keys are small
//! integers chosen by the game data, so resistance to crafted keys isn't needed).
//! Same mixing as rustc's FxHasher.

use std::hash::{BuildHasherDefault, Hasher};

#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

const K: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl FxHasher {
    #[inline]
    fn add(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(K);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.add(u64::from_le_bytes(c.try_into().expect("8 bytes")));
        }
        for &b in chunks.remainder() {
            self.add(b as u64);
        }
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

pub type FxBuild = BuildHasherDefault<FxHasher>;
pub type FxHashMap<K, V> = std::collections::HashMap<K, V, FxBuild>;
pub type FxHashSet<T> = std::collections::HashSet<T, FxBuild>;

/// Handle -> position lookup for asset banks: a flat table (handles are small and dense),
/// with a map for any stray large handle.
#[derive(Debug, Default, Clone)]
pub struct HandleIndex {
    slots: Vec<u32>,
    far: FxHashMap<u32, u32>,
}

impl HandleIndex {
    #[inline]
    pub fn get(&self, handle: u32) -> Option<usize> {
        match self.slots.get(handle as usize) {
            Some(&i) if i != u32::MAX => Some(i as usize),
            Some(_) => None,
            None => self.far.get(&handle).map(|&i| i as usize),
        }
    }
}

impl FromIterator<(u32, usize)> for HandleIndex {
    fn from_iter<I: IntoIterator<Item = (u32, usize)>>(iter: I) -> Self {
        let mut idx = HandleIndex::default();
        for (h, i) in iter {
            if (h as usize) < (1 << 20) {
                if idx.slots.len() <= h as usize {
                    idx.slots.resize(h as usize + 1, u32::MAX);
                }
                // A repeated handle: the later entry wins, as it did with a HashMap.
                idx.slots[h as usize] = i as u32;
            } else {
                idx.far.insert(h, i as u32);
            }
        }
        idx
    }
}
