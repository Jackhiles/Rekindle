//! The "Date & Time" object (kcclock.mfx): reads the local clock.
//!
//! Expression indices (the object's menu order, verified for 5 = day and 6 = month):
//! 0 hundredths of a second, 1 seconds, 2 minutes, 3 hours, 4 day of week (0 = Sunday),
//! 5 day of month, 6 month (1-12), 7 year.
//! Its stopwatch / countdown actions are not implemented.

use super::Extension;
use crate::ctf_loader::events::Comparison;
use crate::runtime::files::SaveFs;
use crate::runtime::value::Value;
use chrono::{Datelike, Local, Timelike};

#[derive(Default)]
pub struct Clock;

impl Extension for Clock {
    fn condition(
        &self,
        _idx: usize,
        _args: &[Value],
        _cmp: Comparison,
        _fs: &SaveFs,
    ) -> Option<bool> {
        None
    }

    fn action(&mut self, idx: usize, _args: &[Value], _fs: &SaveFs) -> bool {
        // 8 / 9 are called together at frame start with no parameters: the stopwatch
        // reset / start actions. Nothing reads the stopwatch, so they need no state.
        matches!(idx, 8 | 9)
    }

    fn expression(&self, idx: usize, _args: &[Value], _fs: &SaveFs) -> Option<Value> {
        let now = Local::now();
        let v = match idx {
            0 => (now.timestamp_subsec_millis() / 10) as i32,
            1 => now.second() as i32,
            2 => now.minute() as i32,
            3 => now.hour() as i32,
            4 => now.weekday().num_days_from_sunday() as i32,
            5 => now.day() as i32,
            6 => now.month() as i32,
            7 => now.year(),
            _ => return None,
        };
        Some(Value::Int(v))
    }
}
