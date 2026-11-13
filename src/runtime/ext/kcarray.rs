//! The standard "Array" object (kcarray.mfx): a 3-D table of numbers or strings.
//!
//! Object data: `x_dim i32, y_dim i32, z_dim i32, flags i32` (1 = numeric, 2 = text,
//! 4 = indexes start at 1). Index mapping (the object's menu order, matching how games
//! call it): conditions 0..2 "index X/Y/Z at end"; actions 0..2 set index X/Y/Z, 3..5 add 1
//! to index X/Y/Z, 6 write value, 7 write string, 8 clear, 9 load, 10 load (selector),
//! 11 save, 12 save (selector), 13..15 write value at X / X,Y / X,Y,Z, 16..18 write string
//! at X / X,Y / X,Y,Z; expressions 0..2 index X/Y/Z, 3 read value, 4 read string,
//! 5..7 read value at X / X,Y / X,Y,Z, 8..10 read string at X / X,Y / X,Y,Z, 11..13
//! dimension X/Y/Z.
//!
//! Files: `"CNC ARRAY\0"`, version u16 (2), x, y, z dimensions and flags (u32 each),
//! then the cells in x-fastest order: i32 numbers, or (u32 length, bytes) strings.

use super::{arg_i, arg_s, Extension};
use crate::ctf_loader::events::Comparison;
use crate::runtime::files::SaveFs;
use crate::runtime::value::Value;

const NUMERIC: u32 = 1;
const BASE1: u32 = 4;
const MAGIC: &[u8] = b"CNC ARRAY\0";

pub struct KcArray {
    dims: [usize; 3],
    flags: u32,
    nums: Vec<i32>,
    strs: Vec<String>,
    /// Current indexes (as the game sees them).
    pos: [i32; 3],
}

impl KcArray {
    pub fn from_data(data: Option<&[u8]>) -> Self {
        let rd = |i: usize| -> u32 {
            data.and_then(|d| d.get(i * 4..i * 4 + 4)).map_or(0, |b| u32::from_le_bytes(b.try_into().expect("4 bytes")))
        };
        let dims = [rd(0), rd(1), rd(2)].map(|d| (d as usize).clamp(1, 1 << 20));
        let flags = if data.is_some() { rd(3) } else { NUMERIC };
        let mut a = Self { dims, flags, nums: Vec::new(), strs: Vec::new(), pos: [0; 3] };
        a.clear();
        let b = a.base();
        a.pos = [b; 3];
        a
    }

    fn numeric(&self) -> bool {
        self.flags & NUMERIC != 0
    }

    fn base(&self) -> i32 {
        (self.flags & BASE1 != 0) as i32
    }

    fn cells(&self) -> usize {
        self.dims.iter().product::<usize>().min(1 << 24)
    }

    fn clear(&mut self) {
        let n = self.cells();
        if self.numeric() {
            self.nums = vec![0; n];
            self.strs.clear();
        } else {
            self.strs = vec![String::new(); n];
            self.nums.clear();
        }
    }

    /// Cell index for game coordinates, growing the array if needed (like the original).
    fn cell(&mut self, x: i32, y: i32, z: i32) -> Option<usize> {
        let b = self.base();
        let c = [x - b, y - b, z - b];
        if c.iter().any(|&v| v < 0) {
            return None;
        }
        let c = c.map(|v| v as usize);
        if (0..3).any(|k| c[k] >= self.dims[k]) {
            if c.iter().any(|&v| v >= 1 << 16) {
                return None;
            }
            self.grow([c[0] + 1, c[1] + 1, c[2] + 1]);
        }
        Some(c[0] + self.dims[0] * (c[1] + self.dims[1] * c[2]))
    }

    fn peek(&self, x: i32, y: i32, z: i32) -> Option<usize> {
        let b = self.base();
        let c = [x - b, y - b, z - b];
        if (0..3).any(|k| c[k] < 0 || c[k] as usize >= self.dims[k]) {
            return None;
        }
        Some(c[0] as usize + self.dims[0] * (c[1] as usize + self.dims[1] * c[2] as usize))
    }

    fn grow(&mut self, want: [usize; 3]) {
        let nd = [0, 1, 2].map(|k| self.dims[k].max(want[k]));
        let (od, numeric) = (self.dims, self.numeric());
        let mut nums = vec![0; if numeric { nd.iter().product() } else { 0 }];
        let mut strs = vec![String::new(); if numeric { 0 } else { nd.iter().product() }];
        for z in 0..od[2] {
            for y in 0..od[1] {
                for x in 0..od[0] {
                    let (o, n) = (x + od[0] * (y + od[1] * z), x + nd[0] * (y + nd[1] * z));
                    if numeric {
                        nums[n] = self.nums[o];
                    } else {
                        strs[n] = std::mem::take(&mut self.strs[o]);
                    }
                }
            }
        }
        self.dims = nd;
        self.nums = nums;
        self.strs = strs;
    }

    fn write(&mut self, at: [i32; 3], v: &Value) {
        let numeric = self.numeric();
        if let Some(i) = self.cell(at[0], at[1], at[2]) {
            if numeric {
                self.nums[i] = v.as_i32();
            } else {
                self.strs[i] = v.as_string();
            }
        }
    }

    fn read(&self, at: [i32; 3]) -> Value {
        match self.peek(at[0], at[1], at[2]) {
            Some(i) if self.numeric() => Value::Int(self.nums[i]),
            Some(i) => Value::Str(self.strs[i].clone()),
            None if self.numeric() => Value::Int(0),
            None => Value::Str(String::new()),
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&2u16.to_le_bytes());
        for d in self.dims {
            out.extend_from_slice(&(d as u32).to_le_bytes());
        }
        out.extend_from_slice(&self.flags.to_le_bytes());
        if self.numeric() {
            for v in &self.nums {
                out.extend_from_slice(&v.to_le_bytes());
            }
        } else {
            for s in &self.strs {
                out.extend_from_slice(&(s.len() as u32).to_le_bytes());
                out.extend_from_slice(s.as_bytes());
            }
        }
        out
    }

    fn from_bytes(&mut self, b: &[u8]) -> bool {
        if !b.starts_with(MAGIC) || b.len() < MAGIC.len() + 18 {
            return false;
        }
        let mut p = MAGIC.len() + 2;
        let u32_at = |p: &mut usize| -> Option<u32> {
            let v = u32::from_le_bytes(b.get(*p..*p + 4)?.try_into().ok()?);
            *p += 4;
            Some(v)
        };
        let (Some(x), Some(y), Some(z), Some(flags)) = (u32_at(&mut p), u32_at(&mut p), u32_at(&mut p), u32_at(&mut p)) else {
            return false;
        };
        self.dims = [x, y, z].map(|d| (d as usize).clamp(1, 1 << 20));
        self.flags = flags;
        self.clear();
        let n = self.cells();
        for i in 0..n {
            if self.numeric() {
                let Some(v) = u32_at(&mut p) else { break };
                self.nums[i] = v as i32;
            } else {
                let Some(len) = u32_at(&mut p) else { break };
                let Some(s) = b.get(p..p + len as usize) else { break };
                self.strs[i] = String::from_utf8_lossy(s).into_owned();
                p += len as usize;
            }
        }
        true
    }
}

impl Extension for KcArray {
    fn condition(&self, idx: usize, _args: &[Value], _cmp: Comparison, _fs: &SaveFs) -> Option<bool> {
        let b = self.base();
        match idx {
            0..=2 => Some(self.pos[idx] - b >= self.dims[idx] as i32 - 1),
            _ => None,
        }
    }

    fn action(&mut self, idx: usize, args: &[Value], fs: &SaveFs) -> bool {
        let p = self.pos;
        match idx {
            0..=2 => self.pos[idx] = arg_i(args, 0),
            3..=5 => self.pos[idx - 3] += 1,
            6 | 7 => {
                let v = args.first().cloned().unwrap_or(Value::Int(0));
                self.write(p, &v);
            }
            8 => self.clear(),
            9 | 10 => {
                if let Some(b) = fs.read(&arg_s(args, 0)) {
                    self.from_bytes(&b);
                }
            }
            11 | 12 => {
                let _ = fs.write(&arg_s(args, 0), &self.to_bytes());
            }
            13..=18 => {
                let v = args.first().cloned().unwrap_or(Value::Int(0));
                let n = (idx - 13) % 3 + 1;
                let mut at = p;
                for k in 0..n {
                    at[k] = arg_i(args, k + 1);
                }
                self.write(at, &v);
            }
            19..=21 => self.pos[idx - 19] -= 1,
            _ => return false,
        }
        true
    }

    fn expression(&self, idx: usize, args: &[Value], _fs: &SaveFs) -> Option<Value> {
        let p = self.pos;
        Some(match idx {
            0..=2 => Value::Int(self.pos[idx]),
            3 | 4 => self.read(p),
            5..=10 => {
                let n = (idx - 5) % 3 + 1;
                let mut at = p;
                for k in 0..n {
                    at[k] = arg_i(args, k);
                }
                let v = self.read(at);
                // 5..7 read values, 8..10 strings.
                if idx >= 8 { Value::Str(v.as_string()) } else { v }
            }
            11..=13 => Value::Int(self.dims[idx - 11] as i32),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_and_round_trips() {
        let mut a = KcArray::from_data(Some(&[2, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0]));
        a.write([5, 1, 0], &Value::Str("hi".into()));
        assert_eq!(a.read([5, 1, 0]).as_string(), "hi");
        assert_eq!(a.dims, [6, 2, 1]);
        let bytes = a.to_bytes();
        let mut b = KcArray::from_data(None);
        assert!(b.from_bytes(&bytes));
        assert_eq!(b.read([5, 1, 0]).as_string(), "hi");
    }
}
