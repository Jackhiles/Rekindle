//! The standard "Ini" object (kcini.mfx): reads and writes Windows-style INI files.
//!
//! Index mapping (from the object's menu order, verified against how games call it):
//! actions 0 set group, 1 set item, 2 set value, 3/4 save/load object position (unsupported),
//! 5 set string, 6 set current file, 7 set value (item), 8 set value (group, item),
//! 9 set string (item), 10 set string (group, item), 11 delete item, 12 delete (group, item),
//! 13 delete group; expressions 0 value, 1 string, 2 value (item), 3 value (group, item),
//! 4 string (item), 5 string (group, item).

use super::{arg_i, arg_s, Extension};
use crate::ctf_loader::events::Comparison;
use crate::runtime::files::SaveFs;
use crate::runtime::value::Value;

#[derive(Default)]
pub struct Ini {
    /// Game-visible path of the current file.
    path: Option<String>,
    group: String,
    item: String,
    /// (group, [(item, value)]) in file order.
    data: Vec<(String, Vec<(String, String)>)>,
}

fn parse(text: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut out: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            out.push((name.to_string(), Vec::new()));
        } else if let Some((k, v)) = line.split_once('=') {
            if out.is_empty() {
                out.push((String::new(), Vec::new()));
            }
            out.last_mut()
                .expect("non-empty")
                .1
                .push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    out
}

impl Ini {
    /// File names without a folder live in %APPDATA%\MMFApplications, like the original object.
    fn resolve(name: &str, fs: &SaveFs) -> String {
        if name.contains(['\\', '/', ':']) {
            name.to_string()
        } else {
            let appdata = fs
                .appdata
                .as_ref()
                .map_or_else(String::new, |a| a.to_string_lossy().into_owned());
            format!("{appdata}\\MMFApplications\\{name}")
        }
    }

    fn load(&mut self, fs: &SaveFs) {
        self.data = self
            .path
            .as_ref()
            .and_then(|p| fs.read(p))
            .map(|b| parse(&String::from_utf8_lossy(&b)))
            .unwrap_or_default();
    }

    fn save(&self, fs: &SaveFs) {
        let Some(p) = &self.path else { return };
        let mut text = String::new();
        for (g, items) in &self.data {
            text.push_str(&format!("[{g}]\r\n"));
            for (k, v) in items {
                text.push_str(&format!("{k}={v}\r\n"));
            }
        }
        if let Err(e) = fs.write(p, text.as_bytes()) {
            eprintln!("ini: cannot write {p}: {e}");
        }
    }

    fn get(&self, group: &str, item: &str) -> Option<&str> {
        let (_, items) = self
            .data
            .iter()
            .find(|(g, _)| g.eq_ignore_ascii_case(group))?;
        items
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(item))
            .map(|(_, v)| v.as_str())
    }

    fn set(&mut self, group: &str, item: &str, value: String, fs: &SaveFs) {
        let gi = match self
            .data
            .iter()
            .position(|(g, _)| g.eq_ignore_ascii_case(group))
        {
            Some(i) => i,
            None => {
                self.data.push((group.to_string(), Vec::new()));
                self.data.len() - 1
            }
        };
        let items = &mut self.data[gi].1;
        match items.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(item)) {
            Some(entry) => entry.1 = value,
            None => items.push((item.to_string(), value)),
        }
        self.save(fs);
    }

    fn delete(&mut self, group: &str, item: Option<&str>, fs: &SaveFs) {
        match item {
            Some(item) => {
                if let Some((_, items)) = self
                    .data
                    .iter_mut()
                    .find(|(g, _)| g.eq_ignore_ascii_case(group))
                {
                    items.retain(|(k, _)| !k.eq_ignore_ascii_case(item));
                }
            }
            None => self.data.retain(|(g, _)| !g.eq_ignore_ascii_case(group)),
        }
        self.save(fs);
    }

    fn value(&self, group: &str, item: &str) -> Value {
        let s = self.get(group, item).unwrap_or("0").trim();
        match s.parse::<i32>() {
            Ok(i) => Value::Int(i),
            Err(_) => Value::Float(s.parse::<f64>().unwrap_or(0.0)),
        }
    }

    fn string(&self, group: &str, item: &str) -> Value {
        Value::Str(self.get(group, item).unwrap_or("").to_string())
    }
}

impl Extension for Ini {
    fn condition(
        &self,
        _idx: usize,
        _args: &[Value],
        _cmp: Comparison,
        _fs: &SaveFs,
    ) -> Option<bool> {
        None
    }

    fn action(&mut self, idx: usize, args: &[Value], fs: &SaveFs) -> bool {
        let (g, i) = (self.group.clone(), self.item.clone());
        match idx {
            0 => self.group = arg_s(args, 0),
            1 => self.item = arg_s(args, 0),
            2 => self.set(&g, &i, arg_i(args, 0).to_string(), fs),
            5 => self.set(&g, &i, arg_s(args, 0), fs),
            6 => {
                self.path = Some(Self::resolve(&arg_s(args, 0), fs));
                self.load(fs);
            }
            7 => self.set(&g, &arg_s(args, 0), arg_i(args, 1).to_string(), fs),
            8 => self.set(
                &arg_s(args, 0),
                &arg_s(args, 1),
                arg_i(args, 2).to_string(),
                fs,
            ),
            9 => self.set(&g, &arg_s(args, 0), arg_s(args, 1), fs),
            10 => self.set(&arg_s(args, 0), &arg_s(args, 1), arg_s(args, 2), fs),
            11 => self.delete(&g, Some(&arg_s(args, 0)), fs),
            12 => self.delete(&arg_s(args, 0), Some(&arg_s(args, 1)), fs),
            13 => self.delete(&arg_s(args, 0), None, fs),
            _ => return false,
        }
        true
    }

    fn expression(&self, idx: usize, args: &[Value], _fs: &SaveFs) -> Option<Value> {
        let (g, i) = (&self.group, &self.item);
        Some(match idx {
            0 => self.value(g, i),
            1 => self.string(g, i),
            2 => self.value(g, &arg_s(args, 0)),
            3 => self.value(&arg_s(args, 0), &arg_s(args, 1)),
            4 => self.string(g, &arg_s(args, 0)),
            5 => self.string(&arg_s(args, 0), &arg_s(args, 1)),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_lookup_is_case_insensitive() {
        let ini = Ini {
            data: parse("[Game]\r\nmoney=150\r\nname=Bob\r\n"),
            group: "game".into(),
            ..Default::default()
        };
        assert_eq!(ini.value("game", "MONEY"), Value::Int(150));
        assert_eq!(ini.string("GAME", "name"), Value::Str("Bob".into()));
        assert_eq!(ini.value("GAME", "missing"), Value::Int(0));
    }
}
