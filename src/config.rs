use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
pub struct Rule {
    pub pattern: String,
    pub layer: u8,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DeviceRef {
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial_number: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    pub rules: Vec<Rule>,
    pub device: Option<DeviceRef>,
    /// Layer to fall back to when the focused window matches no rule.
    #[serde(default)]
    pub default_layer: u8,
    /// Skip showing the main window on launch. Only applied when autostart
    /// is also enabled — a manual launch should always show the window.
    #[serde(default)]
    pub start_minimized: bool,
}

fn config_path() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "layerhook")?;
    Some(dirs.config_dir().join("config.json"))
}

impl AppConfig {
    pub fn load() -> Self {
        config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = config_path() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }
}
