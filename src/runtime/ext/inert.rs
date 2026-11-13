//! Extensions that only talk to the operating system or a platform service and have no
//! visible effect inside the game: window maximise / restore (`kcwctrl`), Discord status
//! (`DRPC`), hidden editor properties, and mobile / console helpers (`Android`,
//! `InAppAndroid`, `XNA`) that PC builds still contain. Actions are accepted and do
//! nothing; conditions are false ("not on that platform", "not maximised"); expressions
//! are 0 or "".

use super::Extension;
use crate::ctf_loader::events::Comparison;
use crate::runtime::files::SaveFs;
use crate::runtime::value::Value;

pub struct Inert;

impl Extension for Inert {
    fn condition(&self, _idx: usize, _args: &[Value], _cmp: Comparison, _fs: &SaveFs) -> Option<bool> {
        Some(false)
    }

    fn action(&mut self, _idx: usize, _args: &[Value], _fs: &SaveFs) -> bool {
        true
    }

    fn expression(&self, _idx: usize, _args: &[Value], _fs: &SaveFs) -> Option<Value> {
        Some(Value::Int(0))
    }
}
