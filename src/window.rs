#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::{list_window_titles, owner_note, watch};
#[cfg(target_os = "windows")]
pub use windows::{list_window_titles, owner_note, watch};
