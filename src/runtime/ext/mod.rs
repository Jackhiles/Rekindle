//! Extension objects (.mfx) reimplemented natively.
//!
//! An extension's conditions, actions and expressions are numbered per extension:
//! condition `num` -81 is its condition 0, action 80 is action 0, expression 80 is
//! expression 0. Each supported extension maps those indices to behaviour here.

mod clock;
mod gsx;
mod ini;

use super::files::SaveFs;
use super::value::Value;
use crate::ctf_loader::events::Comparison;

pub trait Extension {
    /// `None` = this condition isn't implemented.
    fn condition(&self, idx: usize, args: &[Value], cmp: Comparison, fs: &SaveFs) -> Option<bool>;
    /// `false` = this action isn't implemented.
    fn action(&mut self, idx: usize, args: &[Value], fs: &SaveFs) -> bool;
    fn expression(&self, idx: usize, args: &[Value], fs: &SaveFs) -> Option<Value>;
}

/// State shared by all objects of an extension (per running game).
pub struct Shared {
    gsx: gsx::Store,
}

impl Default for Shared {
    fn default() -> Self {
        Self { gsx: gsx::new_store() }
    }
}

/// Create the native implementation for an extension file name (e.g. "kcini"), if we have one.
pub fn create(name: &str, shared: &Shared) -> Option<Box<dyn Extension>> {
    match name.to_ascii_lowercase().as_str() {
        "kcini" => Some(Box::new(ini::Ini::default())),
        "gstorex" => Some(Box::new(gsx::GlobalStore::with_store(shared.gsx.clone()))),
        "kcclock" => Some(Box::new(clock::Clock)),
        _ => None,
    }
}

/// Argument helpers.
pub(crate) fn arg_i(args: &[Value], i: usize) -> i32 {
    args.get(i).map_or(0, Value::as_i32)
}

pub(crate) fn arg_s(args: &[Value], i: usize) -> String {
    args.get(i).map(Value::as_string).unwrap_or_default()
}
