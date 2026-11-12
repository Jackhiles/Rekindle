//! Fusion's dynamically typed expression values.

use crate::ctf_loader::events::Comparison;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i32),
    Float(f64),
    Str(String),
}

impl Default for Value {
    fn default() -> Self {
        Value::Int(0)
    }
}

impl Value {
    pub fn as_f64(&self) -> f64 {
        match self {
            Value::Int(i) => *i as f64,
            Value::Float(f) => *f,
            Value::Str(s) => s.trim().parse().unwrap_or(0.0),
        }
    }

    pub fn as_i32(&self) -> i32 {
        match self {
            Value::Int(i) => *i,
            Value::Float(f) => *f as i32,
            Value::Str(_) => self.as_f64() as i32,
        }
    }

    pub fn as_string(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            other => other.to_string(),
        }
    }

    pub fn is_str(&self) -> bool {
        matches!(self, Value::Str(_))
    }

    /// Floats with no fractional part stay floats; ints stay ints.
    pub fn number(f: f64, as_float: bool) -> Value {
        if as_float {
            Value::Float(f)
        } else {
            Value::Int(f as i32)
        }
    }

    fn any_float(a: &Value, b: &Value) -> bool {
        matches!(a, Value::Float(_)) || matches!(b, Value::Float(_))
    }

    pub fn add(&self, b: &Value) -> Value {
        if self.is_str() || b.is_str() {
            return Value::Str(self.as_string() + &b.as_string());
        }
        match (self, b) {
            (Value::Int(x), Value::Int(y)) => Value::Int(x.wrapping_add(*y)),
            _ => Value::Float(self.as_f64() + b.as_f64()),
        }
    }

    pub fn arith(&self, op: i16, b: &Value) -> Value {
        let fl = Value::any_float(self, b);
        match (self, b, op) {
            (Value::Int(x), Value::Int(y), 4) => Value::Int(x.wrapping_sub(*y)),
            (Value::Int(x), Value::Int(y), 6) => Value::Int(x.wrapping_mul(*y)),
            (Value::Int(x), Value::Int(y), 8) => {
                Value::Int(if *y == 0 { 0 } else { x.wrapping_div(*y) })
            }
            (Value::Int(x), Value::Int(y), 10) => {
                Value::Int(if *y == 0 { 0 } else { x.wrapping_rem(*y) })
            }
            _ => {
                let (x, y) = (self.as_f64(), b.as_f64());
                match op {
                    2 => self.add(b),
                    4 => Value::number(x - y, fl),
                    6 => Value::number(x * y, fl),
                    8 => Value::number(if y == 0.0 { 0.0 } else { x / y }, fl),
                    10 => Value::number(if y == 0.0 { 0.0 } else { x % y }, fl),
                    12 => {
                        let r = x.powf(y);
                        Value::number(r, fl || r.fract() != 0.0)
                    }
                    14 => Value::Int(self.as_i32() & b.as_i32()),
                    16 => Value::Int(self.as_i32() | b.as_i32()),
                    18 => Value::Int(self.as_i32() ^ b.as_i32()),
                    _ => Value::Int(0),
                }
            }
        }
    }

    pub fn compare(&self, cmp: Comparison, b: &Value) -> bool {
        use std::cmp::Ordering::*;
        let ord = if self.is_str() || b.is_str() {
            self.as_string().cmp(&b.as_string())
        } else {
            self.as_f64().partial_cmp(&b.as_f64()).unwrap_or(Equal)
        };
        match cmp {
            Comparison::Equal => ord == Equal,
            Comparison::NotEqual => ord != Equal,
            Comparison::LessEqual => ord != Greater,
            Comparison::Less => ord == Less,
            Comparison::GreaterEqual => ord != Less,
            Comparison::Greater => ord == Greater,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => {
                if x.fract() == 0.0 && x.abs() < 1e15 {
                    write!(f, "{}", *x as i64)
                } else {
                    write!(f, "{x}")
                }
            }
            Value::Str(s) => f.write_str(s),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_division_and_float_promotion() {
        assert_eq!(Value::Int(7).arith(8, &Value::Int(2)), Value::Int(3));
        assert_eq!(
            Value::Int(7).arith(8, &Value::Float(2.0)),
            Value::Float(3.5)
        );
        assert_eq!(Value::Int(1).arith(8, &Value::Int(0)), Value::Int(0));
    }

    #[test]
    fn plus_concatenates_strings() {
        assert_eq!(
            Value::Str("a".into()).add(&Value::Int(1)),
            Value::Str("a1".into())
        );
    }

    #[test]
    fn comparisons() {
        assert!(Value::Int(3).compare(Comparison::Less, &Value::Float(3.5)));
        assert!(Value::Str("b".into()).compare(Comparison::Greater, &Value::Str("a".into())));
    }
}
