//! Expression evaluation. Fusion stores expressions as infix token streams:
//! values, operators (object type 0), `(` `,` `)` (system -1/-2/-3) and function tokens
//! whose arguments follow them and end with `)`.

use super::events::Sel;
use super::value::Value;
use super::Runtime;
use crate::ctf_loader::events::{Token, TokenValue};
use crate::ctf_loader::ObjectKind;

const SYS: i16 = -1;
const OPEN: i16 = -1;
const CLOSE: i16 = -2;
const COMMA: i16 = -3;

fn precedence(op: i16) -> u8 {
    match op {
        12 => 4,         // power
        6 | 8 | 10 => 3, // * / mod
        2 | 4 => 2,      // + -
        _ => 1,          // and / or / xor
    }
}

fn is_close_or_comma(t: &Token) -> bool {
    t.is(SYS, CLOSE) || t.is(SYS, COMMA)
}

pub struct Eval<'a> {
    pub rt: &'a Runtime,
    pub sel: &'a Sel,
    /// The instance an action is currently being applied to (oi, index).
    pub cur: Option<(u16, usize)>,
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Eval<'a> {
    pub fn new(
        rt: &'a Runtime,
        sel: &'a Sel,
        cur: Option<(u16, usize)>,
        tokens: &'a [Token],
    ) -> Self {
        Self {
            rt,
            sel,
            cur,
            tokens,
            pos: 0,
        }
    }

    pub fn run(mut self) -> Value {
        if self.tokens.is_empty() {
            return Value::Int(0);
        }
        self.expr(0)
    }

    fn peek(&self) -> Option<&'a Token> {
        self.tokens.get(self.pos)
    }

    fn expr(&mut self, min_prec: u8) -> Value {
        let mut lhs = self.primary();
        while let Some(t) = self.peek() {
            if !t.is_operator() {
                break;
            }
            let p = precedence(t.num);
            if p < min_prec {
                break;
            }
            self.pos += 1;
            let rhs = self.expr(p + 1);
            lhs = if t.num == 2 {
                lhs.add(&rhs)
            } else {
                lhs.arith(t.num, &rhs)
            };
        }
        lhs
    }

    /// Arguments of a function token, if any follow it.
    fn args(&mut self) -> Vec<Value> {
        let mut out = Vec::new();
        match self.peek() {
            None => return out,
            Some(t) if t.is_operator() || is_close_or_comma(t) => return out,
            _ => {}
        }
        loop {
            out.push(self.expr(0));
            match self.peek() {
                Some(t) if t.is(SYS, COMMA) => self.pos += 1,
                Some(t) if t.is(SYS, CLOSE) => {
                    self.pos += 1;
                    break;
                }
                _ => break,
            }
        }
        out
    }

    fn primary(&mut self) -> Value {
        let Some(t) = self.peek() else {
            return Value::Int(0);
        };
        self.pos += 1;
        match (t.object_type, t.num, &t.value) {
            (SYS, 0, TokenValue::Long(v)) => Value::Int(*v),
            (SYS, 23, TokenValue::Double(v)) => Value::Float(*v),
            (SYS, 3, TokenValue::String(s)) => Value::Str(s.clone()),
            (SYS, OPEN, _) => {
                let v = self.expr(0);
                if self.peek().is_some_and(|t| t.is(SYS, CLOSE)) {
                    self.pos += 1;
                }
                v
            }
            _ => {
                let args = self.args();
                self.call(t, &args)
            }
        }
    }

    fn call(&self, t: &Token, a: &[Value]) -> Value {
        let rt = self.rt;
        let f = |i: usize| a.get(i).map(Value::as_f64).unwrap_or(0.0);
        let s = |i: usize| a.get(i).map(Value::as_string).unwrap_or_default();
        let fl = Value::Float;
        match t.object_type {
            SYS => match t.num {
                1 => Value::Int(rt.random(f(0).max(1.0) as u32) as i32),
                2 => rt.global_value((f(0) as i64 - 1).max(0) as usize),
                24 => match t.value {
                    TokenValue::Index(i) => rt.global_value(i.max(0) as usize),
                    _ => Value::Int(0),
                },
                49 => Value::Str(rt.global_string((f(0) as i64 - 1).max(0) as usize)),
                50 => match t.value {
                    TokenValue::Index(i) => Value::Str(rt.global_string(i.max(0) as usize)),
                    _ => Value::Str(String::new()),
                },
                4 => Value::Str(a.first().cloned().unwrap_or_default().to_string()),
                6 => Value::Str(rt.files.app_drive()),
                7 => Value::Str(rt.files.app_directory()),
                8 => Value::Str(rt.files.app_path()),
                9 => Value::Str(rt.files.app_file.to_string_lossy().into_owned()),
                56 | 57 => Value::Str(std::env::temp_dir().to_string_lossy().into_owned()),
                5 => {
                    let v = s(0).trim().parse::<f64>().unwrap_or(0.0);
                    Value::number(v, v.fract() != 0.0)
                }
                10 => fl(f(0).to_radians().sin()),
                11 => fl(f(0).to_radians().cos()),
                12 => fl(f(0).to_radians().tan()),
                13 => fl(f(0).sqrt()),
                14 => fl(f(0).log10()),
                15 => fl(f(0).ln()),
                16 => Value::Str(format!("0x{:X}", f(0) as i64)),
                17 => Value::Str(format!("0b{:b}", f(0) as i64)),
                18 => fl(f(0).exp()),
                19 => Value::Str(s(0).chars().take(f(1).max(0.0) as usize).collect()),
                20 => {
                    let st = s(0);
                    let n = f(1).max(0.0) as usize;
                    let len = st.chars().count();
                    Value::Str(st.chars().skip(len.saturating_sub(n)).collect())
                }
                21 => Value::Str(
                    s(0).chars()
                        .skip(f(1).max(0.0) as usize)
                        .take(f(2).max(0.0) as usize)
                        .collect(),
                ),
                22 => Value::Int(s(0).chars().count() as i32),
                28 => Value::Int(f(0) as i32),
                29 => match a.first() {
                    Some(Value::Int(i)) => Value::Int(i.wrapping_abs()),
                    _ => fl(f(0).abs()),
                },
                30 => Value::Int(f(0).ceil() as i32),
                31 => Value::Int(f(0).floor() as i32),
                32 => fl(f(0).acos().to_degrees()),
                33 => fl(f(0).asin().to_degrees()),
                34 => fl(f(0).atan().to_degrees()),
                35 => Value::Int(!(f(0) as i32)),
                40 => {
                    if f(0) <= f(1) {
                        a[0].clone()
                    } else {
                        a.get(1).cloned().unwrap_or_default()
                    }
                }
                41 => {
                    if f(0) >= f(1) {
                        a[0].clone()
                    } else {
                        a.get(1).cloned().unwrap_or_default()
                    }
                }
                42 => Value::Int(
                    (f(0) as i32 & 255) | (f(1) as i32 & 255) << 8 | (f(2) as i32 & 255) << 16,
                ),
                43 => Value::Int(f(0) as i32 & 255),
                44 => Value::Int((f(0) as i32 >> 8) & 255),
                45 => Value::Int((f(0) as i32 >> 16) & 255),
                46 => Value::Int(rt.loop_index(&s(0))),
                47 => Value::Str("\r\n".into()),
                48 => Value::Int(f(0).round() as i32),
                51 => Value::Str(s(0).to_lowercase()),
                52 => Value::Str(s(0).to_uppercase()),
                53 => Value::Int(
                    s(0).get(f(2).max(0.0) as usize..)
                        .and_then(|h| h.find(&s(1)))
                        .map_or(-1, |p| p as i32 + f(2) as i32),
                ),
                54 => Value::Int(s(0).rfind(&s(1)).map_or(-1, |p| p as i32)),
                58 => Value::Str(format!("{}", f(0))),
                59 => fl(f(0).atan2(f(1)).to_degrees()),
                60 => Value::Int(0),
                61 => Value::Str(String::new()),
                62 => fl(((f(2) - f(0)).powi(2) + (f(3) - f(1)).powi(2)).sqrt()),
                63 => fl((-(f(3) - f(1)))
                    .atan2(f(2) - f(0))
                    .to_degrees()
                    .rem_euclid(360.0)),
                64 => fl(f(0).clamp(f(1).min(f(2)), f(2).max(f(1)))),
                65 => {
                    let (lo, hi) = (f(0) as i64, f(1) as i64);
                    let span = (hi - lo + 1).max(1) as u32;
                    Value::Int((lo + rt.random(span) as i64) as i32)
                }
                n => rt.note_unimplemented(format!("expr -1/{n}"), Value::Int(0)),
            },
            -3 => match t.num {
                2 => fl(rt.scroll.0),
                3 => fl(rt.scroll.0 + rt.view_size.0),
                4 => fl(rt.scroll.1),
                5 => fl(rt.scroll.1 + rt.view_size.1),
                6 => Value::Int(rt.frame().width),
                7 => Value::Int(rt.frame().height),
                0 | 8 => Value::Int(rt.frame_index as i32 + 1),
                10 => Value::Int(rt.frame_rate as i32),
                n => rt.note_unimplemented(format!("expr -3/{n}"), Value::Int(0)),
            },
            -4 => {
                let ms = rt.timer_ms;
                match t.num {
                    0 => Value::Int(ms as i32),
                    1 => Value::Int(((ms / 10.0) as i32) % 100),
                    2 => Value::Int(((ms / 1000.0) as i32) % 60),
                    3 => Value::Int((ms / 3_600_000.0) as i32),
                    4 => Value::Int(((ms / 60_000.0) as i32) % 60),
                    _ => Value::Int(0),
                }
            }
            -5 => Value::Int(rt.instances.iter().filter(|i| !i.destroyed).count() as i32),
            -2 => {
                let a0 = f(0).max(0.0) as usize;
                match t.num {
                    0 => Value::Int(rt.audio.main_volume as i32),
                    3 => Value::Int(rt.audio.main_pan as i32),
                    5 => Value::Int(rt.audio.channel_pan[a0.min(super::audio::CHANNELS)] as i32),
                    2 => Value::Int(rt.audio.channel_volume[a0.min(super::audio::CHANNELS)] as i32),
                    7 => Value::Int(
                        rt.audio
                            .channel_sample(a0)
                            .map_or(0.0, |h| rt.audio.position_ms(h))
                            as i32,
                    ),
                    9 => Value::Int(
                        rt.audio
                            .channel_sample(a0)
                            .and_then(|h| rt.audio.durations.get(&h))
                            .copied()
                            .unwrap_or(0.0) as i32,
                    ),
                    n => rt.note_unimplemented(format!("expr Sound/{n}"), Value::Int(0)),
                }
            }
            -6 => match t.num {
                0 => Value::Int(rt.input_mouse.0 as i32),
                1 => Value::Int(rt.input_mouse.1 as i32),
                _ => Value::Int(0),
            },
            -7 => match t.num {
                0 => Value::Int(rt.score[super::events::player_index(t.object_info)]),
                1 => Value::Int(rt.lives[super::events::player_index(t.object_info)]),
                _ => Value::Int(0),
            },
            ot if ot >= 2 => self.object_expr(t, a),
            _ => rt.note_unimplemented(format!("expr {}/{}", t.object_type, t.num), Value::Int(0)),
        }
    }

    fn object_expr(&self, t: &Token, a: &[Value]) -> Value {
        let rt = self.rt;
        let oi = t.object_info;
        if t.num == 15 {
            return Value::Int(rt.candidates(oi, self.sel).len() as i32);
        }
        if let ObjectKind::Extension(_) = rt
            .game
            .object(oi)
            .map_or(ObjectKind::Unknown(0), |o| o.kind)
        {
            if t.num >= 80 && rt.oi_has_widget(oi) {
                let Some(idx) = rt.pick(oi, self.sel, self.cur) else {
                    return Value::Int(0);
                };
                let r = rt.instances[idx].widget.as_ref().and_then(|w| w.expression((t.num - 80) as usize, a));
                return r.unwrap_or_else(|| {
                    rt.note_unimplemented(format!("expr {}/{}", rt.ace_owner_label(t.object_type), t.num), Value::Int(0))
                });
            }
            if t.num >= 80 {
                let r = rt.ext_expression(oi, (t.num - 80) as usize, a);
                return r.unwrap_or_else(|| {
                    rt.note_unimplemented(
                        format!("expr {}/{}", rt.ace_owner_label(t.object_type), t.num),
                        Value::Int(0),
                    )
                });
            }
        }
        let Some(idx) = rt.pick(oi, self.sel, self.cur) else {
            return Value::Int(0);
        };
        let inst = &rt.instances[idx];
        let index = || match t.value {
            TokenValue::Index(i) => i.max(0) as usize,
            _ => 0,
        };
        let b = || rt.bounds_of(idx);
        match (inst.kind, t.num) {
            (_, 1) => Value::Int(inst.y as i32),
            (_, 11) => Value::Int(inst.x as i32),
            (_, 2) => Value::Int(inst.anim.forced_frame.unwrap_or(inst.anim.frame) as i32),
            (_, 3) => Value::Int(inst.current_speed() as i32),
            (_, 6) => Value::Int(inst.direction() as i32),
            (_, 7) => Value::Int(b().0 as i32),
            (_, 8) => Value::Int((b().0 + b().2) as i32),
            (_, 9) => Value::Int(b().1 as i32),
            (_, 10) => Value::Int((b().1 + b().3) as i32),
            (_, 12) => Value::Int(inst.id as i32),
            (_, 13) => {
                Value::Int((inst.flags >> (a.first().map_or(0, Value::as_i32) & 31) & 1) as i32)
            }
            (_, 14) => Value::Int(inst.current_anim_id() as i32),
            (_, 16) => inst.value(index()),
            (_, 17) => Value::Int(((255 - inst.tint[3] as i32) * 128 / 255).clamp(0, 128)),
            (_, 19) => Value::Str(inst.string(index())),
            (_, 23) => Value::Int(inst.layer as i32 + 1),
            // Action point of the current image, in frame coordinates.
            (_, 25) | (_, 26) => {
                let ap = self.rt.game.object(inst.oi).and_then(|o| inst.current_image(o)).and_then(|h| self.rt.game.images.get(h));
                let (x, y) = match ap {
                    Some(e) => (
                        inst.x + (e.action_point.0 - e.hotspot.0) as f64 * inst.scale_x,
                        inst.y + (e.action_point.1 - e.hotspot.1) as f64 * inst.scale_y,
                    ),
                    None => (inst.x, inst.y),
                };
                Value::Int(if t.num == 25 { x } else { y } as i32)
            }
            (_, 27) => Value::Int(255 - inst.tint[3] as i32),
            (_, 30) => inst.value(a.first().map_or(0, Value::as_i32).max(0) as usize),
            (_, 31) => Value::Str(inst.string(a.first().map_or(0, Value::as_i32).max(0) as usize)),
            (_, 40) => Value::Int(b().2 as i32),
            (_, 41) => Value::Int(b().3 as i32),
            (ObjectKind::Active, 81) => Value::Float(inst.scale_x),
            (ObjectKind::Active, 82) => Value::Float(inst.scale_y),
            (ObjectKind::Active, 83) => Value::Float(inst.angle),
            (ObjectKind::Counter, 80) => inst.counter.clone(),
            (ObjectKind::Counter, 81) => Value::Int(inst.counter_min as i32),
            (ObjectKind::Counter, 82) => Value::Int(inst.counter_max as i32),
            (ObjectKind::Text, 81) => Value::Str(inst.text.clone()),
            (ObjectKind::Text, 83) => Value::Int(inst.text.trim().parse().unwrap_or(0)),
            _ => rt.note_unimplemented(
                format!("expr {}/{}", rt.ace_owner_label(t.object_type), t.num),
                Value::Int(0),
            ),
        }
    }
}
