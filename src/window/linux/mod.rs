//! Active window tracking, event-driven where the desktop supports it.
//!
//! Backend picked once at startup based on the session:
//! - Wayland with `wlr-foreign-toplevel-management` (Hyprland, Sway, River):
//!   push-based, see [`wlr_toplevel`]. Tried first on any Wayland session.
//!   COSMIC's `cosmic-comp` does NOT advertise this (verified live) - falls
//!   through to the next backend there.
//! - Wayland with `ext-foreign-toplevel-list-v1` +
//!   `cosmic-toplevel-info-unstable-v1` (COSMIC): push-based, see
//!   [`cosmic_toplevel`]. Verified live against `cosmic-comp`.
//! - Wayland with KWin's `plasma-window-management` (KDE Plasma): push-based,
//!   see [`plasma_window`]. Verified live against a real KWin session
//!   (v20) - uses the `window_with_uuid`/`get_window_by_uuid` path, the
//!   only one modern KWin actually sends.
//! - X11 (including a plain X11 session on any EWMH window manager): push-
//!   based via `_NET_ACTIVE_WINDOW`, see [`x11`].
//! - Wayland with none of the above advertised (GNOME/Mutter, which
//!   implements neither): unsupported, no backend. We don't fall back to the
//!   X11 path here even though `DISPLAY` is usually also set (XWayland) -
//!   the XWayland root's `_NET_ACTIVE_WINDOW` doesn't reflect native Wayland
//!   client focus, so that fallback would silently report stale/wrong titles
//!   instead of cleanly doing nothing. GNOME needs a Shell extension (e.g.
//!   "Window Calls") exposing this over D-Bus - not wired up here.

mod cosmic_toplevel;
mod plasma_window;
mod wlr_toplevel;
mod x11;

use std::sync::mpsc::Sender;

pub fn watch(tx: Sender<Option<String>>) {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if wlr_toplevel::watch(tx.clone()) {
            return;
        }
        if cosmic_toplevel::watch(tx.clone()) {
            return;
        }
        plasma_window::watch(tx);
        return;
    }
    x11::watch(tx);
}

pub fn list_window_titles() -> Vec<String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if let Some(titles) = wlr_toplevel::list_window_titles() {
            return titles;
        }
        if let Some(titles) = cosmic_toplevel::list_window_titles() {
            return titles;
        }
        return plasma_window::list_window_titles().unwrap_or_default();
    }
    x11::list_window_titles()
}
