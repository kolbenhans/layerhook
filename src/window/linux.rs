use std::process::Command;

fn hyprctl_json(args: &[&str]) -> Option<serde_json::Value> {
    let out = Command::new("hyprctl").args(args).arg("-j").output().ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

pub fn active_window_title() -> Option<String> {
    let v = hyprctl_json(&["activewindow"])?;
    v.get("title")?.as_str().map(str::to_string)
}

pub fn list_window_titles() -> Vec<String> {
    let Some(v) = hyprctl_json(&["clients"]) else {
        return Vec::new();
    };
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c.get("title")?.as_str().map(str::to_string))
        .filter(|t| !t.is_empty())
        .collect()
}
