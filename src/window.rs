//! Active window title tracking, per platform. `watch()` is push-based
//! (spawns whatever the platform needs, sends a title update whenever focus
//! changes) rather than polled - see the platform modules for why.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::{list_window_titles, owner_note, watch};
#[cfg(target_os = "windows")]
pub use windows::{list_window_titles, owner_note, watch};
