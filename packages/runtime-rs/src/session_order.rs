use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct SessionOrder {
    order: Vec<String>,
    hidden: Vec<String>,
    persist_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct PersistedSessionOrder {
    order: Option<Value>,
    hidden: Option<Value>,
}

impl SessionOrder {
    pub fn new(persist_path: Option<PathBuf>) -> Self {
        let mut session_order = Self {
            order: Vec::new(),
            hidden: Vec::new(),
            persist_path,
        };
        session_order.load();
        session_order
    }

    pub fn sync(&mut self, names: impl IntoIterator<Item = String>) {
        let names = names.into_iter().collect::<Vec<_>>();
        let name_set = names.iter().cloned().collect::<BTreeSet<_>>();
        self.order.retain(|name| name_set.contains(name));
        self.hidden.retain(|name| name_set.contains(name));
        for name in names {
            if !self.order.contains(&name) {
                self.order.push(name);
            }
        }
    }

    pub fn reorder(&mut self, name: &str, delta: i8) {
        let Some(index) = self.order.iter().position(|candidate| candidate == name) else {
            return;
        };
        let new_index = index as isize + delta as isize;
        if new_index < 0 || new_index >= self.order.len() as isize {
            return;
        }
        self.order.swap(index, new_index as usize);
        let _ = self.save();
    }

    pub fn hide(&mut self, name: &str) {
        if !self.order.iter().any(|candidate| candidate == name)
            || self.hidden.iter().any(|candidate| candidate == name)
        {
            return;
        }
        self.hidden.push(name.to_string());
        let _ = self.save();
    }

    pub fn show(&mut self, name: &str) {
        let len_before = self.hidden.len();
        self.hidden.retain(|candidate| candidate != name);
        if self.hidden.len() == len_before {
            return;
        }
        if !self.order.iter().any(|candidate| candidate == name) {
            self.order.push(name.to_string());
        }
        let _ = self.save();
    }

    pub fn show_all(&mut self) {
        if self.hidden.is_empty() {
            return;
        }
        self.hidden.clear();
        let _ = self.save();
    }

    pub fn apply(&self, names: impl IntoIterator<Item = String>) -> Vec<String> {
        let mut names = names
            .into_iter()
            .filter(|name| !self.hidden.contains(name))
            .collect::<Vec<_>>();
        names.sort_by(|a, b| {
            let pa = self
                .order
                .iter()
                .position(|name| name == a)
                .unwrap_or(usize::MAX);
            let pb = self
                .order
                .iter()
                .position(|name| name == b)
                .unwrap_or(usize::MAX);
            pa.cmp(&pb)
        });
        names
    }

    fn load(&mut self) {
        let Some(path) = &self.persist_path else {
            return;
        };
        let Ok(raw) = fs::read_to_string(path) else {
            return;
        };
        let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
            return;
        };

        if let Value::Array(values) = parsed {
            self.order = strings_from_value_array(values);
            return;
        }

        let Ok(persisted) = serde_json::from_value::<PersistedSessionOrder>(parsed) else {
            return;
        };
        if let Some(Value::Array(order)) = persisted.order {
            self.order = strings_from_value_array(order);
        }
        if let Some(Value::Array(hidden)) = persisted.hidden {
            self.hidden = strings_from_value_array(hidden);
        }
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.persist_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let value = if self.hidden.is_empty() {
            serde_json::json!(self.order)
        } else {
            serde_json::json!({ "order": self.order, "hidden": self.hidden })
        };
        let encoded = serde_json::to_string(&value).map_err(io::Error::other)?;
        fs::write(path, format!("{encoded}\n"))
    }
}

fn strings_from_value_array(values: Vec<Value>) -> Vec<String> {
    values
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}
