//! COSMIC backend, combining two protocols:
//!
//! - `ext-foreign-toplevel-list-v1` (standard, staging): gives the toplevel
//!   list plus title/app_id - but deliberately omits any notion of focus,
//!   by design (privacy: a client shouldn't learn which window is active
//!   just from enumerating open windows).
//! - `cosmic-toplevel-info-unstable-v1` (COSMIC-specific): extends a given
//!   `ext_foreign_toplevel_handle_v1` with the state we actually need -
//!   `activated`, among others - via `get_cosmic_toplevel`.
//!
//! Verified live against `cosmic-comp`: COSMIC does not advertise
//! `zwlr-foreign-toplevel-management` at all (the wlr backend is tried
//! first and cleanly fails to bind there), so this is the dedicated COSMIC
//! path. Push-based, same shape as [`super::wlr_toplevel`].

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
    ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
};
use cosmic_protocols::toplevel_info::v1::client::{
    zcosmic_toplevel_handle_v1::{self, ZcosmicToplevelHandleV1},
    zcosmic_toplevel_info_v1::ZcosmicToplevelInfoV1,
};

// zcosmic_toplevel_handle_v1::state enum, "activated" entry (value 2). The
// `state` event carries these packed as 4-byte LE u32s, same wire format as
// wlr's protocol - see wlr_toplevel.rs's identical decoding.
const STATE_ACTIVATED: u32 = 2;

#[derive(Default)]
struct Win {
    title: Option<String>,
    activated: bool,
}

struct State {
    // Keyed by the ext handle - that's where title/app_id/closed live. The
    // zcosmic handle for the same toplevel carries the ext handle as its
    // user-data (see get_cosmic_toplevel below), so its event handler can
    // look the right Win back up without a second map.
    windows: HashMap<ExtForeignToplevelHandleV1, Win>,
    // None once bound; needed to pair every new ext handle with a
    // zcosmic_toplevel_handle_v1 as toplevels arrive.
    cosmic_mgr: Option<ZcosmicToplevelInfoV1>,
    // None for the one-off list_window_titles() snapshot, which doesn't need
    // to push anything.
    tx: Option<Sender<Option<String>>>,
    last_sent: Option<String>,
}

impl State {
    fn active_title(&self) -> Option<String> {
        self.windows.values().find(|w| w.activated).and_then(|w| w.title.clone())
    }

    fn maybe_notify(&mut self) {
        let Some(tx) = &self.tx else { return };
        let current = self.active_title();
        if current != self.last_sent {
            let _ = tx.send(current.clone());
            self.last_sent = current;
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

// No events from this one are useful to us at the version we bind (>=2):
// `toplevel`/`finished` are only sent to v1 clients, and the `done` batching
// signal isn't needed since we notify eagerly per-event, same as
// plasma_window.rs. Still required: wayland-client demands a Dispatch impl
// for every interface type a client binds, even an empty one.
impl Dispatch<ZcosmicToplevelInfoV1, ()> for State {
    fn event(_: &mut Self, _: &ZcosmicToplevelInfoV1, _: cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_info_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ExtForeignToplevelListV1, ()> for State {
    fn event(state: &mut Self, _list: &ExtForeignToplevelListV1, event: ext_foreign_toplevel_list_v1::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            state.windows.insert(toplevel.clone(), Win::default());
            if let Some(mgr) = &state.cosmic_mgr {
                // User-data = the ext handle itself, so the zcosmic handle's
                // state events can find their way back to the right Win
                // without a second lookup map.
                mgr.get_cosmic_toplevel(&toplevel, qh, toplevel.clone());
            }
        }
    }

    // The `toplevel` event (opcode 0) carries a new_id - wayland-client
    // needs to know the child object's user-data type before it can even
    // parse the event, hence this instead of just handling it in `event()`.
    fn event_created_child(opcode: u16, qhandle: &QueueHandle<Self>) -> std::sync::Arc<dyn wayland_client::backend::ObjectData> {
        match opcode {
            0 => qhandle.make_data::<ExtForeignToplevelHandleV1, ()>(()),
            _ => unreachable!("ext_foreign_toplevel_list_v1 has only one event with a new_id"),
        }
    }
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for State {
    fn event(state: &mut Self, handle: &ExtForeignToplevelHandleV1, event: ext_foreign_toplevel_handle_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use ext_foreign_toplevel_handle_v1::Event;
        match event {
            Event::Title { title } => {
                state.windows.entry(handle.clone()).or_default().title = Some(title);
                state.maybe_notify();
            }
            Event::Closed => {
                state.windows.remove(handle);
                state.maybe_notify();
            }
            _ => {}
        }
    }
}

// User-data on the zcosmic handle is the ext handle it extends (see
// get_cosmic_toplevel above) - that's how its state events find their Win.
impl Dispatch<ZcosmicToplevelHandleV1, ExtForeignToplevelHandleV1> for State {
    fn event(state: &mut Self, _handle: &ZcosmicToplevelHandleV1, event: zcosmic_toplevel_handle_v1::Event, ext_handle: &ExtForeignToplevelHandleV1, _: &Connection, _: &QueueHandle<Self>) {
        if let zcosmic_toplevel_handle_v1::Event::State { state: raw } = event {
            let activated = raw.chunks_exact(4).any(|c| u32::from_ne_bytes(c.try_into().unwrap()) == STATE_ACTIVATED);
            if let Some(win) = state.windows.get_mut(ext_handle) {
                win.activated = activated;
            }
            state.maybe_notify();
        }
    }
}

fn bind_managers(qh: &QueueHandle<State>, globals: &wayland_client::globals::GlobalList) -> Option<ZcosmicToplevelInfoV1> {
    // Version 2 is the minimum that gets us get_cosmic_toplevel and the
    // non-deprecated `state` event; version 1 only has the legacy path this
    // backend doesn't use.
    globals.bind::<ExtForeignToplevelListV1, _, _>(qh, 1..=1, ()).ok()?;
    globals.bind::<ZcosmicToplevelInfoV1, _, _>(qh, 2..=3, ()).ok()
}

/// Spawns the watcher thread if the compositor advertises both protocols.
/// Returns false (does nothing) otherwise - caller decides the fallback.
pub fn watch(tx: Sender<Option<String>>) -> bool {
    let Ok(conn) = Connection::connect_to_env() else { return false };
    let Ok((globals, mut event_queue)) = registry_queue_init::<State>(&conn) else { return false };
    let qh = event_queue.handle();
    let Some(cosmic_mgr) = bind_managers(&qh, &globals) else { return false };

    let mut state = State { windows: HashMap::new(), cosmic_mgr: Some(cosmic_mgr), tx: Some(tx), last_sent: None };
    std::thread::spawn(move || {
        while event_queue.blocking_dispatch(&mut state).is_ok() {}
    });
    true
}

/// One-off snapshot for the "pick from open window" dropdown. None if the
/// compositor doesn't support both protocols.
pub fn list_window_titles() -> Option<Vec<String>> {
    let conn = Connection::connect_to_env().ok()?;
    let (globals, mut event_queue) = registry_queue_init::<State>(&conn).ok()?;
    let qh = event_queue.handle();
    let cosmic_mgr = bind_managers(&qh, &globals)?;

    let mut state = State { windows: HashMap::new(), cosmic_mgr: Some(cosmic_mgr), tx: None, last_sent: None };
    // First roundtrip: the list's `toplevel` events arrive, creating ext
    // handles and (via get_cosmic_toplevel) their paired zcosmic handles.
    // Second: each handle's initial title/state events arrive.
    event_queue.roundtrip(&mut state).ok()?;
    event_queue.roundtrip(&mut state).ok()?;

    Some(state.windows.values().filter_map(|w| w.title.clone()).filter(|t| !t.is_empty()).collect())
}
