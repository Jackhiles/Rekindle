//! "Global Store X" (gstorex.mfx): an array of global integer values with save / load.
//!
//! Index mapping, inferred from how games use it (parameter types, file paths, what the
//! values are compared with) and from a save file it wrote:
//! condition 0: compare value(index) with a value;
//! actions 0 set (index, value), 3 add (index, value), 6 subtract (index, value),
//! 8 set range (from, to, value), 10 clear all, 26 and 29 save to file, 30 and 33 load from file;
//! expression 0: value(index).
//!
//! File layout (little-endian): `u16 3`, then four sections of `u32 count, u8 flag`;
//! the first section is followed by `count` i32 values, the others are empty.

use super::{arg_i, arg_s, Extension};
use crate::ctf_loader::events::Comparison;
use crate::runtime::files::SaveFs;
use crate::runtime::value::Value;
use std::cell::RefCell;
use std::rc::Rc;

const DEFAULT_SIZE: usize = 200;

/// The store is global: every Global Store X object in the game shares it.
pub type Store = Rc<RefCell<Vec<i32>>>;

pub fn new_store() -> Store {
    Rc::new(RefCell::new(vec![0; DEFAULT_SIZE]))
}

pub struct GlobalStore {
    values: Store,
}

impl Default for GlobalStore {
    fn default() -> Self {
        Self { values: new_store() }
    }
}

impl GlobalStore {
    pub fn with_store(values: Store) -> Self {
        Self { values }
    }

    fn get(&self, i: i32) -> i32 {
        usize::try_from(i)
            .ok()
            .and_then(|i| self.values.borrow().get(i).copied())
            .unwrap_or(0)
    }

    fn set(&mut self, i: i32, v: i32) {
        let Ok(i) = usize::try_from(i) else { return };
        if i >= 100_000 {
            return;
        }
        let mut values = self.values.borrow_mut();
        if i >= values.len() {
            values.resize(i + 1, 0);
        }
        values[i] = v;
    }

    pub fn encode(&self) -> Vec<u8> {
        let values = self.values.borrow();
        let mut out = Vec::with_capacity(22 + values.len() * 4);
        out.extend_from_slice(&3u16.to_le_bytes());
        out.extend_from_slice(&(values.len() as u32).to_le_bytes());
        out.push(0);
        for v in values.iter() {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for _ in 0..3 {
            out.extend_from_slice(&0u32.to_le_bytes());
            out.push(0);
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Vec<i32>> {
        let count = u32::from_le_bytes(bytes.get(2..6)?.try_into().ok()?) as usize;
        let data = bytes.get(7..7 + count * 4)?;
        Some(
            data.chunks_exact(4)
                .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        )
    }
}

impl Extension for GlobalStore {
    fn condition(&self, idx: usize, args: &[Value], cmp: Comparison, _fs: &SaveFs) -> Option<bool> {
        match idx {
            0 => Some(
                Value::Int(self.get(arg_i(args, 0)))
                    .compare(cmp, args.get(1).unwrap_or(&Value::Int(0))),
            ),
            _ => None,
        }
    }

    fn action(&mut self, idx: usize, args: &[Value], fs: &SaveFs) -> bool {
        let (i, v) = (arg_i(args, 0), arg_i(args, 1));
        match idx {
            0 => self.set(i, v),
            3 => self.set(i, self.get(i).wrapping_add(v)),
            6 => self.set(i, self.get(i).wrapping_sub(v)),
            8 => {
                let (from, to, value) = (i.min(v), i.max(v), arg_i(args, 2));
                for k in from..=to.min(from + 100_000) {
                    self.set(k, value);
                }
            }
            10 => self.values.borrow_mut().iter_mut().for_each(|v| *v = 0),
            // 26 and 29: save to file (26 runs on leaving a screen, 29 at frame changes).
            26 | 29 => {
                let path = arg_s(args, 0);
                if let Err(e) = fs.write(&path, &self.encode()) {
                    eprintln!("global store: cannot write {path}: {e}");
                }
            }
            30 | 33 => {
                let path = arg_s(args, 0);
                if let Some(mut values) = fs.read(&path).and_then(|b| Self::decode(&b)) {
                    if values.len() < DEFAULT_SIZE {
                        values.resize(DEFAULT_SIZE, 0);
                    }
                    *self.values.borrow_mut() = values;
                }
            }
            _ => return false,
        }
        true
    }

    fn expression(&self, idx: usize, args: &[Value], _fs: &SaveFs) -> Option<Value> {
        match idx {
            0 => Some(Value::Int(self.get(arg_i(args, 0)))),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_format_round_trips() {
        let mut g = GlobalStore::default();
        g.set(1, 3);
        g.set(124, 30);
        let bytes = g.encode();
        assert_eq!(bytes.len(), 822);
        assert_eq!(&bytes[..7], &[3, 0, 200, 0, 0, 0, 0]);
        let back = GlobalStore::decode(&bytes).unwrap();
        assert_eq!(back[1], 3);
        assert_eq!(back[124], 30);
    }
}
