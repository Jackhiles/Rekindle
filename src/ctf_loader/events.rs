//! Event sheets (frame chunk 0x333D): groups of conditions and actions.
//!
//! Layout: marker blocks ("ER>>", "ERes", "ERev", "<<ER"); the "ERev" block holds
//! `size: u32` bytes of event groups. Each group: `size: i16 (negative), n_conditions: u8,
//! n_actions: u8, flags: u16, ...` then conditions and actions, each `size: u16, object_type: i16,
//! num: i16, object_info: u16, object_info_list: i16, flags: u8, other_flags: u8,
//! n_params: u8, def_type: u8, [identifier: u16 (conditions only)]`, then parameters
//! `size: u16, code: i16, data`.

use super::reader::ByteReader;
use anyhow::{bail, Result};

/// Group flags.
pub mod group_flags {
    pub const ONCE: u16 = 0x0001;
    pub const NOT_ALWAYS: u16 = 0x0002;
    pub const REPEAT: u16 = 0x0004;
    pub const NO_MORE: u16 = 0x0008;
    pub const OR_IN_GROUP: u16 = 0x0400;
    pub const OR_LOGICAL: u16 = 0x1000;
    pub const INACTIVE: u16 = 0x4000;
}

/// Condition/action `other_flags`: bit 0 negates a condition.
pub const ACE_NOT: u8 = 0x01;

/// A comparison operator used by expression parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparison {
    Equal,
    NotEqual,
    LessEqual,
    Less,
    GreaterEqual,
    Greater,
}

impl Comparison {
    pub fn from_i16(v: i16) -> Self {
        match v {
            1 => Self::NotEqual,
            2 => Self::LessEqual,
            3 => Self::Less,
            4 => Self::GreaterEqual,
            5 => Self::Greater,
            _ => Self::Equal,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Position {
    /// 0xFFFF = absolute (frame coordinates).
    pub parent_object_info: u16,
    pub flags: u16,
    pub x: i16,
    pub y: i16,
    pub slope: i16,
    pub angle: i16,
    pub direction: i32,
    pub parent_type: i16,
    pub object_info_list: i16,
    pub layer: i16,
}

pub mod position_flags {
    /// Transform the offset by the parent's direction.
    pub const DIRECTION: u16 = 0x1;
    pub const ACTION_POINT: u16 = 0x2;
    pub const INITIAL_DIRECTION: u16 = 0x4;
    pub const DEFAULT_DIRECTION: u16 = 0x8;
}

/// One expression token. Expressions are stored in infix order.
#[derive(Debug, Clone)]
pub struct Token {
    pub object_type: i16,
    pub num: i16,
    pub object_info: u16,
    pub object_info_list: i16,
    pub value: TokenValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenValue {
    None,
    Long(i32),
    Double(f64),
    String(String),
    /// Global value / string index, or alterable value / string index.
    Index(i16),
}

impl Token {
    pub fn is_operator(&self) -> bool {
        self.object_type == 0 && self.num != 0
    }
    pub fn is(&self, object_type: i16, num: i16) -> bool {
        self.object_type == object_type && self.num == num
    }
}

#[derive(Debug, Clone)]
pub enum Param {
    Object {
        object_info_list: i16,
        object_info: u16,
        object_type: i16,
    },
    Time {
        ms: i32,
        loops: i32,
    },
    Short(i16),
    Int(i32),
    Sample {
        handle: i16,
        flags: u16,
        name: String,
    },
    Create {
        position: Position,
        instance: u16,
        object_info: u16,
    },
    Shoot {
        position: Position,
        instance: u16,
        object_info: u16,
        speed: i16,
    },
    Position(Position),
    Key(u16),
    Click {
        button: u8,
        double: bool,
    },
    Zone {
        x1: i16,
        y1: i16,
        x2: i16,
        y2: i16,
    },
    Every {
        ms: i32,
    },
    Group {
        flags: u16,
        id: u16,
        name: String,
    },
    GroupPointer {
        id: i16,
    },
    Expression {
        comparison: Comparison,
        tokens: Vec<Token>,
    },
    CompareTime {
        ms: i32,
        loops: i32,
        comparison: Comparison,
    },
    String(String),
    Colour([u8; 4]),
    TwoShorts(i16, i16),
    /// Extension-specific or not yet understood.
    Raw(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct ParamEntry {
    pub code: i16,
    pub value: Param,
}

/// A condition or action ("ACE").
#[derive(Debug, Clone)]
pub struct Ace {
    pub object_type: i16,
    pub num: i16,
    pub object_info: u16,
    pub object_info_list: i16,
    pub flags: u8,
    pub other_flags: u8,
    pub def_type: u8,
    pub identifier: u16,
    pub params: Vec<ParamEntry>,
}

impl Ace {
    pub fn negated(&self) -> bool {
        self.other_flags & ACE_NOT != 0
    }
}

#[derive(Debug, Clone)]
pub struct EventGroup {
    pub flags: u16,
    pub conditions: Vec<Ace>,
    pub actions: Vec<Ace>,
}

#[derive(Debug, Clone, Default)]
pub struct Events {
    pub groups: Vec<EventGroup>,
}

/// `old`: Multimedia Fusion 1.5 layout (byte-sized object types / numbers).
pub fn parse_events(data: &[u8], unicode: bool, old: bool) -> Result<Events> {
    let Some(start) = data.windows(4).position(|w| w == b"ERev") else {
        return Ok(Events::default());
    };
    let mut r = ByteReader::new(data);
    r.seek(start + 4)?;
    let size = r.read_u32()? as usize;
    let end = (r.pos() + size).min(data.len());
    let mut groups = Vec::new();
    if !old && is_compact(data, r.pos(), end) {
        return parse_compact_groups(data, r.pos(), end, unicode);
    }
    while r.pos() < end {
        let gstart = r.pos();
        let gsize = (r.read_i16()? as i32).unsigned_abs() as usize;
        if gsize == 0 {
            bail!("zero-sized event group at {gstart:#x}");
        }
        let nc = r.read_u8()? as usize;
        let na = r.read_u8()? as usize;
        let flags = r.read_u16()?;
        let gend = gstart + gsize;
        // The group header is 14 bytes in older runtimes and 16 in newer ones;
        // pick the one where the ACE sizes add up to the group size.
        let header = [14usize, 16]
            .into_iter()
            .find(|&h| ace_chain_end(data, gstart + h, nc + na, old) == Some(gend))
            .unwrap_or(14);
        r.seek(gstart + header)?;
        let mut conditions = Vec::with_capacity(nc);
        let mut actions = Vec::with_capacity(na);
        for i in 0..nc + na {
            let ace = parse_ace(data, r.pos(), i < nc, unicode, old)?;
            let sz = u16::from_le_bytes([data[r.pos()], data[r.pos() + 1]]) as usize;
            r.seek(r.pos() + sz)?;
            if i < nc {
                conditions.push(ace);
            } else {
                actions.push(ace);
            }
        }
        groups.push(EventGroup {
            flags,
            conditions,
            actions,
        });
        r.seek(gend.min(end))?;
    }
    Ok(Events { groups })
}

// ---------- compact event format (newer Fusion 2.5+ builds) ----------
//
// Group header: `size i16 (negative), n_conditions u8, n_actions u8, flags u16, id u16`.
// ACE: `size u16, object_type i16, num i16`, then `object_info u16` for objects and the
// player type (-7), then `flags u8` (bit 7 = negated) and `n_params u8`, then the usual
// parameters. A few common "value vs constant" ACEs have no parameters and store their
// operands inline instead; those are turned back into ordinary parameters here.

fn compact_has_oi(object_type: i16) -> bool {
    object_type >= 0 || object_type == -7
}

/// End of a compact ACE's own data (header plus parameters), if it looks valid.
fn compact_ace_end(data: &[u8], pos: usize) -> Option<(usize, usize)> {
    let size = u16::from_le_bytes([*data.get(pos)?, *data.get(pos + 1)?]) as usize;
    let ot = i16::from_le_bytes([*data.get(pos + 2)?, *data.get(pos + 3)?]);
    let head = if compact_has_oi(ot) { 10 } else { 8 };
    let n = *data.get(pos + head - 1)? as usize;
    let mut p = pos + head;
    for _ in 0..n {
        let psz = u16::from_le_bytes([*data.get(p)?, *data.get(p + 1)?]) as usize;
        if psz < 4 {
            return None;
        }
        p += psz;
    }
    (size >= head && p <= pos + size).then_some((pos + size, p))
}

/// Do all groups in `start..end` add up under the compact layout (and not all under the
/// classic one)? Single groups can fit both by coincidence.
fn is_compact(data: &[u8], start: usize, end: usize) -> bool {
    fits_compact(data, start, end) && !fits_classic(data, start, end)
}

fn fits_classic(data: &[u8], start: usize, end: usize) -> bool {
    let mut g = start;
    while g + 4 <= end {
        let gsize = (i16::from_le_bytes([data[g], data[g + 1]]) as i32).unsigned_abs() as usize;
        let n = data[g + 2] as usize + data[g + 3] as usize;
        if gsize == 0 || ![14usize, 16].iter().any(|&h| ace_chain_end(data, g + h, n, false) == Some(g + gsize)) {
            return false;
        }
        g += gsize;
    }
    true
}

fn fits_compact(data: &[u8], start: usize, end: usize) -> bool {
    let mut g = start;
    let mut checked = 0;
    while g < end {
        let Some(&[a, b, nc, na]) = data.get(g..g + 4).map(|s| <&[u8; 4]>::try_from(s).unwrap()) else {
            return false;
        };
        let gsize = (i16::from_le_bytes([a, b]) as i32).unsigned_abs() as usize;
        if gsize < 8 {
            return false;
        }
        let mut q = g + 8;
        for _ in 0..nc as usize + na as usize {
            match compact_ace_end(data, q) {
                Some((next, _)) => q = next,
                None => return false,
            }
        }
        if q != g + gsize {
            return false;
        }
        g += gsize;
        checked += 1;
    }
    checked > 0
}

fn parse_compact_groups(data: &[u8], start: usize, end: usize, unicode: bool) -> Result<Events> {
    let mut groups = Vec::new();
    let mut g = start;
    while g + 8 <= end {
        let mut r = ByteReader::new(data);
        r.seek(g)?;
        let gsize = (r.read_i16()? as i32).unsigned_abs() as usize;
        if gsize == 0 {
            bail!("zero-sized event group at {g:#x}");
        }
        let nc = r.read_u8()? as usize;
        let na = r.read_u8()? as usize;
        let flags = r.read_u16()?;
        let mut q = g + 8;
        let mut conditions = Vec::with_capacity(nc);
        let mut actions = Vec::with_capacity(na);
        for i in 0..nc + na {
            let (ace, next) = parse_compact_ace(data, q, i < nc, unicode)?;
            q = next;
            if i < nc {
                conditions.push(ace);
            } else {
                actions.push(ace);
            }
        }
        groups.push(EventGroup { flags, conditions, actions });
        g += gsize;
    }
    Ok(Events { groups })
}

fn long_expr(comparison: Comparison, v: Inline) -> Param {
    let (num, value) = match v {
        Inline::Int(i) => (0, TokenValue::Long(i)),
        Inline::Float(f) => (23, TokenValue::Double(f)),
    };
    Param::Expression {
        comparison,
        tokens: vec![Token { object_type: -1, num, object_info: 0, object_info_list: 0, value }],
    }
}

#[derive(Clone, Copy)]
enum Inline {
    Int(i32),
    Float(f64),
}

fn parse_compact_ace(data: &[u8], pos: usize, condition: bool, unicode: bool) -> Result<(Ace, usize)> {
    let mut r = ByteReader::new(data);
    r.seek(pos)?;
    let size = r.read_u16()? as usize;
    let object_type = r.read_i16()?;
    let num = r.read_i16()?;
    let object_info = if compact_has_oi(object_type) { r.read_u16()? } else { 0 };
    let f = r.read_u8()?;
    let n = r.read_u8()? as usize;
    let mut ace = Ace {
        object_type,
        num,
        object_info,
        object_info_list: -1,
        flags: f & 0x7F,
        other_flags: if f & 0x80 != 0 { ACE_NOT } else { 0 },
        def_type: 0,
        identifier: 0,
        params: Vec::new(),
    };
    let end = pos + size;
    for _ in 0..n {
        let pstart = r.pos();
        if pstart + 4 > end {
            break;
        }
        let psize = r.read_u16()? as usize;
        let code = r.read_i16()?;
        let body = data.get(pstart + 4..pstart + psize.max(4)).unwrap_or_default();
        let value = parse_param(code, body, unicode, false).unwrap_or_else(|_| Param::Raw(body.to_vec()));
        ace.params.push(ParamEntry { code, value });
        r.seek(pstart + psize.max(4))?;
    }
    // Inline operands: `index u32`, [`comparison u16`], then an i32 (or f64) constant.
    let inline = data.get(r.pos()..end).unwrap_or_default();
    if n == 0 && inline.len() >= 8 {
        let index = i32::from_le_bytes(inline[0..4].try_into().unwrap());
        let value_at = |o: usize| -> Inline {
            if inline.len() >= o + 8 && inline.len() - o == 8 {
                Inline::Float(f64::from_le_bytes(inline[o..o + 8].try_into().unwrap()))
            } else {
                Inline::Int(i32::from_le_bytes(inline[o..o + 4].try_into().unwrap()))
            }
        };
        let global_index = || ParamEntry {
            code: 22,
            value: long_expr(Comparison::Equal, Inline::Int(index)),
        };
        let alt_index = || ParamEntry { code: 50, value: Param::Short(index as i16) };
        match (object_type, num, condition) {
            // Compare global value with a constant; the comparison is in the number.
            (-1, -39..=-28, true) => {
                let cmp = Comparison::from_i16(((-28 - num) % 6) as i16);
                ace.params.push(global_index());
                ace.params.push(ParamEntry { code: 23, value: long_expr(cmp, value_at(4)) });
            }
            // Set / add / subtract a global value.
            (-1, 27..=38, false) => {
                ace.params.push(global_index());
                ace.params.push(ParamEntry { code: 22, value: long_expr(Comparison::Equal, value_at(4)) });
            }
            // Compare an alterable value with a constant.
            (t, -42, true) if t >= 2 && inline.len() >= 10 => {
                let cmp = Comparison::from_i16(i16::from_le_bytes([inline[4], inline[5]]));
                ace.params.push(alt_index());
                ace.params.push(ParamEntry { code: 23, value: long_expr(cmp, value_at(6)) });
            }
            // Set / add / subtract an alterable value.
            (t, 31..=33, false) if t >= 2 => {
                ace.params.push(alt_index());
                ace.params.push(ParamEntry { code: 22, value: long_expr(Comparison::Equal, value_at(4)) });
            }
            _ => ace.params.push(ParamEntry { code: 0, value: Param::Raw(inline.to_vec()) }),
        }
    }
    Ok((ace, end))
}

fn ace_chain_end(data: &[u8], mut pos: usize, n: usize, old: bool) -> Option<usize> {
    for _ in 0..n {
        let sz = u16::from_le_bytes([*data.get(pos)?, *data.get(pos + 1)?]) as usize;
        if sz < if old { 12 } else { 14 } {
            return None;
        }
        pos += sz;
    }
    Some(pos)
}

fn parse_ace(data: &[u8], pos: usize, condition: bool, unicode: bool, old: bool) -> Result<Ace> {
    let mut r = ByteReader::new(data);
    r.seek(pos)?;
    let size = r.read_u16()? as usize;
    let (object_type, num) = if old {
        let t = r.read_i8()? as i16;
        let mut n = r.read_i8()? as i16;
        // 1.5 numbered type-specific ACEs 32 lower than 2.x does.
        if t >= 2 && condition && n < -48 {
            n -= 32;
        } else if t >= 2 && !condition && n >= 48 {
            n += 32;
        }
        (t, n)
    } else {
        (r.read_i16()?, r.read_i16()?)
    };
    let mut ace = Ace {
        object_type,
        num,
        object_info: r.read_u16()?,
        object_info_list: r.read_i16()?,
        flags: r.read_u8()?,
        other_flags: r.read_u8()?,
        def_type: 0,
        identifier: 0,
        params: Vec::new(),
    };
    let n = r.read_u8()? as usize;
    ace.def_type = r.read_u8()?;
    if condition {
        ace.identifier = r.read_u16()?;
    }
    let end = pos + size;
    for _ in 0..n {
        let pstart = r.pos();
        if pstart + 4 > end {
            break;
        }
        let psize = r.read_u16()? as usize;
        let code = r.read_i16()?;
        let body = data
            .get(pstart + 4..pstart + psize.max(4))
            .unwrap_or_default();
        let value =
            parse_param(code, body, unicode, old).unwrap_or_else(|_| Param::Raw(body.to_vec()));
        ace.params.push(ParamEntry { code, value });
        r.seek(pstart + psize.max(4))?;
    }
    Ok(ace)
}

fn read_position(r: &mut ByteReader) -> Result<Position> {
    Ok(Position {
        parent_object_info: r.read_u16()?,
        flags: r.read_u16()?,
        x: r.read_i16()?,
        y: r.read_i16()?,
        slope: r.read_i16()?,
        angle: r.read_i16()?,
        direction: r.read_i32()?,
        parent_type: r.read_i16()?,
        object_info_list: r.read_i16()?,
        layer: r.read_i16()?,
    })
}

fn parse_param(code: i16, b: &[u8], unicode: bool, old: bool) -> Result<Param> {
    let mut r = ByteReader::new(b);
    if is_expression_code(code) {
        let comparison = Comparison::from_i16(r.read_i16()?);
        return Ok(Param::Expression {
            comparison,
            tokens: parse_tokens(&mut r, unicode, old)?,
        });
    }
    Ok(match code {
        1 => Param::Object {
            object_info_list: r.read_i16()?,
            object_info: r.read_u16()?,
            object_type: r.read_i16()?,
        },
        2 => Param::Time {
            ms: r.read_i32()?,
            loops: r.read_i32()?,
        },
        3 | 4 | 10 | 11 | 12 | 17 | 26 | 31 | 43 | 49 | 50 | 57 | 58 | 60 | 61 => {
            Param::Short(r.read_i16()?)
        }
        5 | 25 | 29 | 34 | 48 | 56 => Param::Int(r.read_i32()?),
        6 | 7 | 35 | 36 => Param::Sample {
            handle: r.read_i16()?,
            flags: r.read_u16()?,
            name: r.read_cstring(unicode).unwrap_or_default(),
        },
        9 | 21 => {
            let position = read_position(&mut r)?;
            Param::Create {
                position,
                instance: r.read_u16()?,
                object_info: r.read_u16()?,
            }
        }
        13 => Param::Every { ms: r.read_i32()? },
        14 | 44 => Param::Key(r.read_u16()?),
        16 => Param::Position(read_position(&mut r)?),
        18 => {
            let position = read_position(&mut r)?;
            let instance = r.read_u16()?;
            let object_info = r.read_u16()?;
            r.skip(4)?;
            Param::Shoot {
                position,
                instance,
                object_info,
                speed: r.read_i16()?,
            }
        }
        19 => Param::Zone {
            x1: r.read_i16()?,
            y1: r.read_i16()?,
            x2: r.read_i16()?,
            y2: r.read_i16()?,
        },
        24 => Param::Colour(r.read_color()?),
        32 => Param::Click {
            button: r.read_u8()?,
            double: r.read_u8()? != 0,
        },
        38 => Param::Group {
            flags: r.read_u16()?,
            id: r.read_u16()?,
            name: r.read_cstring(unicode).unwrap_or_default(),
        },
        39 => {
            let _pointer = r.read_i32()?;
            Param::GroupPointer { id: r.read_i16()? }
        }
        40 | 41 | 63 | 64 => Param::String(r.read_cstring(unicode)?),
        42 => Param::CompareTime {
            ms: r.read_i32()?,
            loops: r.read_i32()?,
            comparison: Comparison::from_i16(r.read_i16()?),
        },
        47 | 51 => Param::TwoShorts(r.read_i16()?, r.read_i16()?),
        _ => Param::Raw(b.to_vec()),
    })
}

fn is_expression_code(code: i16) -> bool {
    matches!(
        code,
        15 | 22 | 23 | 27 | 28 | 45 | 46 | 52 | 53 | 54 | 59 | 62
    )
}

fn parse_tokens(r: &mut ByteReader, unicode: bool, old: bool) -> Result<Vec<Token>> {
    let mut out = Vec::new();
    loop {
        let start = r.pos();
        let (object_type, num) = if old {
            let t = r.read_i8()? as i16;
            let mut n = r.read_i8()? as i16;
            if t > 2 && n >= 48 {
                n += 32;
            }
            (t, n)
        } else {
            (r.read_i16()?, r.read_i16()?)
        };
        if object_type == 0 && num == 0 {
            break;
        }
        let size = r.read_u16()? as usize;
        let mut t = Token {
            object_type,
            num,
            object_info: 0,
            object_info_list: 0,
            value: TokenValue::None,
        };
        match (object_type, num) {
            (-1, 0) => t.value = TokenValue::Long(r.read_i32()?),
            (-1, 3) => t.value = TokenValue::String(r.read_cstring(unicode)?),
            (-1, 23) => {
                t.value =
                    TokenValue::Double(f64::from_le_bytes(r.read_bytes(8)?.try_into().unwrap()))
            }
            (-1, 24) | (-1, 50) => {
                r.skip(4)?;
                t.value = TokenValue::Index(r.read_i16()?);
            }
            (ot, n) if ot >= 2 || ot == -7 => {
                t.object_info = r.read_u16()?;
                t.object_info_list = r.read_i16()?;
                if n == 16 || n == 19 {
                    t.value = TokenValue::Index(r.read_i16()?);
                }
            }
            _ => {}
        }
        out.push(t);
        // A token is at least its header: type + num (1 + 1 bytes in 1.5, 2 + 2 later) + size.
        if size < if old { 4 } else { 6 } {
            bail!("bad expression token size {size}");
        }
        r.seek(start + size)?;
    }
    Ok(out)
}
