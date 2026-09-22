//! X11 backend via EWMH (`_NET_ACTIVE_WINDOW`), push-based through
//! `PropertyNotify`. Covers i3, XFCE, MATE, and KDE/GNOME X11 sessions -
//! effectively any EWMH-compliant window manager.

use std::sync::mpsc::Sender;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ChangeWindowAttributesAux, ConnectionExt, EventMask, Window};
use x11rb::protocol::Event;

struct Atoms {
    net_active_window: u32,
    net_wm_name: u32,
    utf8_string: u32,
    wm_name: u32,
    net_client_list: u32,
}

fn intern_atoms(conn: &impl Connection) -> Option<Atoms> {
    let net_active_window = conn.intern_atom(false, b"_NET_ACTIVE_WINDOW").ok()?.reply().ok()?.atom;
    let net_wm_name = conn.intern_atom(false, b"_NET_WM_NAME").ok()?.reply().ok()?.atom;
    let utf8_string = conn.intern_atom(false, b"UTF8_STRING").ok()?.reply().ok()?.atom;
    let wm_name = AtomEnum::WM_NAME.into();
    let net_client_list = conn.intern_atom(false, b"_NET_CLIENT_LIST").ok()?.reply().ok()?.atom;
    Some(Atoms { net_active_window, net_wm_name, utf8_string, wm_name, net_client_list })
}

fn window_title(conn: &impl Connection, atoms: &Atoms, win: Window) -> Option<String> {
    // Prefer the UTF-8 EWMH name, fall back to the legacy WM_NAME (Latin-1).
    if let Ok(reply) = conn.get_property(false, win, atoms.net_wm_name, atoms.utf8_string, 0, u32::MAX).ok()?.reply() {
        if !reply.value.is_empty() {
            return String::from_utf8(reply.value).ok();
        }
    }
    let reply = conn.get_property(false, win, atoms.wm_name, AtomEnum::STRING, 0, u32::MAX).ok()?.reply().ok()?;
    if reply.value.is_empty() {
        return None;
    }
    Some(reply.value.iter().map(|&b| b as char).collect())
}

fn active_window(conn: &impl Connection, atoms: &Atoms, root: Window) -> Option<Window> {
    let reply = conn.get_property(false, root, atoms.net_active_window, AtomEnum::WINDOW, 0, 1).ok()?.reply().ok()?;
    let win = reply.value32()?.next()?;
    if win == 0 {
        None
    } else {
        Some(win)
    }
}

pub fn watch(tx: Sender<Option<String>>) {
    std::thread::spawn(move || {
        let Ok((conn, screen_num)) = x11rb::connect(None) else { return };
        let Some(atoms) = intern_atoms(&conn) else { return };
        let root = conn.setup().roots[screen_num].root;

        let watch_props = ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE);
        if conn.change_window_attributes(root, &watch_props).is_err() {
            return;
        }
        let _ = conn.flush();

        let mut current_win: Option<Window> = None;
        let mut last_sent: Option<String> = None;
        let send_if_changed = |title: Option<String>, last_sent: &mut Option<String>| {
            if title != *last_sent {
                let _ = tx.send(title.clone());
                *last_sent = title;
            }
        };

        // Pick up whatever's already focused at startup.
        if let Some(win) = active_window(&conn, &atoms, root) {
            current_win = Some(win);
            let _ = conn.change_window_attributes(win, &watch_props);
            let _ = conn.flush();
            send_if_changed(window_title(&conn, &atoms, win), &mut last_sent);
        }

        while let Ok(event) = conn.wait_for_event() {
            let Event::PropertyNotify(e) = event else { continue };
            if e.window == root && e.atom == atoms.net_active_window {
                let win = active_window(&conn, &atoms, root);
                if win != current_win {
                    current_win = win;
                    if let Some(w) = win {
                        // Also watch this window's own title changes (e.g. a
                        // browser tab switch) while it stays focused.
                        let _ = conn.change_window_attributes(w, &watch_props);
                        let _ = conn.flush();
                    }
                }
                let title = win.and_then(|w| window_title(&conn, &atoms, w));
                send_if_changed(title, &mut last_sent);
            } else if Some(e.window) == current_win && (e.atom == atoms.net_wm_name || e.atom == atoms.wm_name) {
                let title = window_title(&conn, &atoms, e.window);
                send_if_changed(title, &mut last_sent);
            }
        }
    });
}

pub fn list_window_titles() -> Vec<String> {
    let Ok((conn, screen_num)) = x11rb::connect(None) else { return Vec::new() };
    let Some(atoms) = intern_atoms(&conn) else { return Vec::new() };
    let root = conn.setup().roots[screen_num].root;

    let Some(reply) = conn.get_property(false, root, atoms.net_client_list, AtomEnum::WINDOW, 0, u32::MAX).ok().and_then(|c| c.reply().ok()) else {
        return Vec::new();
    };
    let Some(windows) = reply.value32() else { return Vec::new() };

    windows.filter_map(|w| window_title(&conn, &atoms, w)).filter(|t| !t.is_empty()).collect()
}
