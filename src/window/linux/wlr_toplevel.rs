use std::collections::HashMap;
use std::sync::mpsc::Sender;

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

#[derive(Default)]
struct Toplevel {
    title: Option<String>,
    activated: bool,
}

struct State {
    toplevels: HashMap<ZwlrForeignToplevelHandleV1, Toplevel>,
    tx: Option<Sender<Option<String>>>,
    last_sent: Option<String>,
}

impl State {
    fn active_title(&self) -> Option<String> {
        self.toplevels.values().find(|t| t.activated).and_then(|t| t.title.clone())
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

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(state: &mut Self, _mgr: &ZwlrForeignToplevelManagerV1, event: zwlr_foreign_toplevel_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            state.toplevels.insert(toplevel, Toplevel::default());
        }
    }

    fn event_created_child(opcode: u16, qhandle: &QueueHandle<Self>) -> std::sync::Arc<dyn wayland_client::backend::ObjectData> {
        match opcode {
            0 => qhandle.make_data::<ZwlrForeignToplevelHandleV1, ()>(()),
            _ => unreachable!("zwlr_foreign_toplevel_manager_v1 has only one event with a new_id"),
        }
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(state: &mut Self, handle: &ZwlrForeignToplevelHandleV1, event: zwlr_foreign_toplevel_handle_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwlr_foreign_toplevel_handle_v1::Event;
        match event {
            Event::Title { title } => {
                state.toplevels.entry(handle.clone()).or_default().title = Some(title);
            }
            Event::State { state: raw } => {
                let activated = raw.chunks_exact(4).any(|c| u32::from_ne_bytes(c.try_into().unwrap()) == zwlr_foreign_toplevel_handle_v1::State::Activated as u32);
                state.toplevels.entry(handle.clone()).or_default().activated = activated;
            }
            Event::Done => state.maybe_notify(),
            Event::Closed => {
                state.toplevels.remove(handle);
                state.maybe_notify();
            }
            _ => {}
        }
    }
}

pub fn watch(tx: Sender<Option<String>>) -> bool {
    let Ok(conn) = Connection::connect_to_env() else { return false };
    let Ok((globals, mut event_queue)) = registry_queue_init::<State>(&conn) else { return false };
    let qh = event_queue.handle();
    if globals.bind::<ZwlrForeignToplevelManagerV1, _, _>(&qh, 1..=3, ()).is_err() {
        return false;
    }

    let mut state = State { toplevels: HashMap::new(), tx: Some(tx), last_sent: None };
    std::thread::spawn(move || {
        while event_queue.blocking_dispatch(&mut state).is_ok() {}
    });
    true
}

pub fn list_window_titles() -> Option<Vec<String>> {
    let conn = Connection::connect_to_env().ok()?;
    let (globals, mut event_queue) = registry_queue_init::<State>(&conn).ok()?;
    let qh = event_queue.handle();
    globals.bind::<ZwlrForeignToplevelManagerV1, _, _>(&qh, 1..=3, ()).ok()?;

    let mut state = State { toplevels: HashMap::new(), tx: None, last_sent: None };

    event_queue.roundtrip(&mut state).ok()?;
    event_queue.roundtrip(&mut state).ok()?;

    Some(state.toplevels.values().filter_map(|t| t.title.clone()).filter(|t| !t.is_empty()).collect())
}
