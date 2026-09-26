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

pub fn owner_note() -> Option<String> {
    None
}
