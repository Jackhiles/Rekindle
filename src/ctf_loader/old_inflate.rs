//! Decompressor for Multimedia Fusion 1.5 data.
//!
//! The format is raw deflate with three differences from RFC 1951:
//! - a block header is a 3-bit type (7 stored, 5 fixed Huffman, 6 dynamic Huffman)
//!   followed by the "final block" bit (deflate puts the final bit first);
//! - stored blocks have a 2-byte length and no one's-complement copy;
//! - dynamic blocks list code-length codes in the order 18, 17, 16, 0, 1, 2 ... 15.

use anyhow::{bail, Result};

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const CODE_LENGTH_ORDER: [usize; 19] = [
    18, 17, 16, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u32,
}

impl Bits<'_> {
    fn get(&mut self, n: u32) -> Result<u32> {
        let mut v = 0;
        for i in 0..n {
            let Some(&byte) = self.data.get(self.pos) else {
                bail!("compressed data ended early")
            };
            v |= ((byte as u32 >> self.bit) & 1) << i;
            self.bit += 1;
            if self.bit == 8 {
                self.bit = 0;
                self.pos += 1;
            }
        }
        Ok(v)
    }

    fn align(&mut self) {
        if self.bit != 0 {
            self.bit = 0;
            self.pos += 1;
        }
    }

    fn consumed(&self) -> usize {
        self.pos + (self.bit != 0) as usize
    }
}

/// Canonical Huffman decoding table.
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Self {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[l as usize] += 1;
        }
        counts[0] = 0;
        let mut offs = [0u16; 16];
        for i in 1..16 {
            offs[i] = offs[i - 1] + counts[i - 1];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offs[l as usize] as usize] = sym as u16;
                offs[l as usize] += 1;
            }
        }
        Self { counts, symbols }
    }

    fn decode(&self, bits: &mut Bits) -> Result<u16> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= bits.get(1)? as i32;
            let count = self.counts[len] as i32;
            if code - count < first {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        bail!("invalid Huffman code")
    }
}

/// Decompress a stream; returns the data and the number of input bytes used.
pub fn inflate(data: &[u8], size_hint: usize) -> Result<(Vec<u8>, usize)> {
    let mut bits = Bits {
        data,
        pos: 0,
        bit: 0,
    };
    let mut out: Vec<u8> = Vec::with_capacity(size_hint);
    loop {
        let kind = bits.get(3)?;
        let last = bits.get(1)? == 1;
        match kind {
            7 => {
                bits.align();
                let p = bits.pos;
                let Some(len) = data
                    .get(p..p + 2)
                    .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
                else {
                    bail!("truncated stored block")
                };
                let Some(chunk) = data.get(p + 2..p + 2 + len) else {
                    bail!("truncated stored block")
                };
                out.extend_from_slice(chunk);
                bits.pos = p + 2 + len;
            }
            5 | 6 => {
                let (lit, dist) = if kind == 5 {
                    let mut l = [0u8; 288];
                    l[..144].fill(8);
                    l[144..256].fill(9);
                    l[256..280].fill(7);
                    l[280..].fill(8);
                    (Huffman::new(&l), Huffman::new(&[5u8; 30]))
                } else {
                    let hlit = bits.get(5)? as usize + 257;
                    let hdist = bits.get(5)? as usize + 1;
                    let hclen = bits.get(4)? as usize + 4;
                    let mut cl = [0u8; 19];
                    for &i in CODE_LENGTH_ORDER.iter().take(hclen) {
                        cl[i] = bits.get(3)? as u8;
                    }
                    let clh = Huffman::new(&cl);
                    let mut lens: Vec<u8> = Vec::with_capacity(hlit + hdist);
                    while lens.len() < hlit + hdist {
                        match clh.decode(&mut bits)? {
                            s @ 0..=15 => lens.push(s as u8),
                            16 => {
                                let Some(&prev) = lens.last() else {
                                    bail!("repeat with no previous length")
                                };
                                let n = 3 + bits.get(2)? as usize;
                                lens.extend(std::iter::repeat_n(prev, n));
                            }
                            17 => {
                                let n = 3 + bits.get(3)? as usize;
                                lens.extend(std::iter::repeat_n(0, n));
                            }
                            _ => {
                                let n = 11 + bits.get(7)? as usize;
                                lens.extend(std::iter::repeat_n(0, n));
                            }
                        }
                    }
                    lens.truncate(hlit + hdist);
                    (Huffman::new(&lens[..hlit]), Huffman::new(&lens[hlit..]))
                };
                loop {
                    let s = lit.decode(&mut bits)? as usize;
                    if s < 256 {
                        out.push(s as u8);
                    } else if s == 256 {
                        break;
                    } else {
                        let i = s - 257;
                        if i >= LEN_BASE.len() {
                            bail!("bad length code");
                        }
                        let len = LEN_BASE[i] as usize + bits.get(LEN_EXTRA[i] as u32)? as usize;
                        let d = dist.decode(&mut bits)? as usize;
                        if d >= DIST_BASE.len() {
                            bail!("bad distance code");
                        }
                        let back = DIST_BASE[d] as usize + bits.get(DIST_EXTRA[d] as u32)? as usize;
                        if back > out.len() {
                            bail!("distance too far back");
                        }
                        let start = out.len() - back;
                        for k in 0..len {
                            out.push(out[start + k]);
                        }
                    }
                }
            }
            k => bail!("unknown block type {k}"),
        }
        if last {
            return Ok((out, bits.consumed()));
        }
    }
}

/// `decompressed_size: u32` followed by a compressed stream.
pub fn inflate_block(b: &[u8]) -> Result<(Vec<u8>, usize)> {
    if b.len() < 4 {
        bail!("compressed block too short");
    }
    let size = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
    let (out, used) = inflate(&b[4..], size)?;
    if out.len() != size {
        bail!("decompressed {} bytes, expected {size}", out.len());
    }
    Ok((out, used + 4))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_block_has_two_byte_length() {
        // type 7 + final bit = 0b1111 in the low bits, then length 3, then data.
        let data = [0x0F, 3, 0, b'a', b'b', b'c'];
        let (out, used) = inflate(&data, 3).unwrap();
        assert_eq!(out, b"abc");
        assert_eq!(used, 6);
    }
}
