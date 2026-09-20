//! Active window title + list of open window titles, per platform.
//! Linux backend is Hyprland-specific (shells out to hyprctl) — matches this
//! project's actual target environment, not a generic X11/Wayland solution.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::{active_window_title, list_window_titles};
#[cfg(target_os = "windows")]
pub use windows::{active_window_title, list_window_titles};
