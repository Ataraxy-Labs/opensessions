use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::protocol::SessionFilterMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarPosition {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OpensessionsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mux: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default)]
    pub plugins: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_width: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_position: Option<SidebarPosition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keybinding: Option<String>,
    #[serde(default)]
    pub detail_panel_heights: BTreeMap<String, u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_filter: Option<SessionFilterMode>,
}

pub fn config_path_from_home(home: &Path) -> PathBuf {
    home.join(".config")
        .join("opensessions")
        .join("config.json")
}

pub fn load_config_from_home(home: &Path) -> OpensessionsConfig {
    let path = config_path_from_home(home);
    let Ok(raw) = fs::read_to_string(path) else {
        return OpensessionsConfig::default();
    };

    let Ok(mut config) = serde_json::from_str::<OpensessionsConfig>(&raw) else {
        return OpensessionsConfig::default();
    };

    if config.plugins.is_empty() {
        config.plugins = Vec::new();
    }

    config
}

pub fn save_config_to_home(home: &Path, updates: OpensessionsConfig) -> io::Result<()> {
    let path = config_path_from_home(home);
    let existing = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({ "plugins": [] }));

    let mut merged = match existing {
        Value::Object(map) => Value::Object(map),
        _ => serde_json::json!({ "plugins": [] }),
    };

    merge_value(&mut merged, Value::Object(update_map(updates)));

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let encoded = serde_json::to_string_pretty(&merged).map_err(io::Error::other)?;
    fs::write(path, format!("{encoded}\n"))
}

fn update_map(updates: OpensessionsConfig) -> Map<String, Value> {
    let mut map = Map::new();

    insert_option(&mut map, "mux", updates.mux);
    insert_option(&mut map, "port", updates.port);
    if !updates.plugins.is_empty() {
        map.insert(
            "plugins".to_string(),
            serde_json::to_value(updates.plugins).expect("plugins serialize"),
        );
    }
    insert_option(&mut map, "theme", updates.theme);
    insert_option(&mut map, "sidebarWidth", updates.sidebar_width);
    insert_option(&mut map, "sidebarPosition", updates.sidebar_position);
    insert_option(&mut map, "keybinding", updates.keybinding);
    if !updates.detail_panel_heights.is_empty() {
        map.insert(
            "detailPanelHeights".to_string(),
            serde_json::to_value(updates.detail_panel_heights).expect("detail heights serialize"),
        );
    }
    insert_option(&mut map, "sessionFilter", updates.session_filter);

    map
}

fn insert_option<T: Serialize>(map: &mut Map<String, Value>, key: &str, value: Option<T>) {
    if let Some(value) = value {
        map.insert(
            key.to_string(),
            serde_json::to_value(value).expect("config value serialize"),
        );
    }
}

fn merge_value(dst: &mut Value, src: Value) {
    match (dst, src) {
        (Value::Object(dst), Value::Object(src)) => {
            for (key, value) in src {
                match dst.get_mut(&key) {
                    Some(existing) => merge_value(existing, value),
                    None => {
                        dst.insert(key, value);
                    }
                }
            }
        }
        (dst, src) => *dst = src,
    }
}
