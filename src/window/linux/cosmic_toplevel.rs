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

const STATE_ACTIVATED: u32 = 2;

#[derive(Default)]
struct Win {
    title: Option<String>,
    activated: bool,
}

struct State {
    windows: HashMap<ExtForeignToplevelHandleV1, Win>,
    cosmic_mgr: Option<ZcosmicToplevelInfoV1>,

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

impl Dispatch<ZcosmicToplevelInfoV1, ()> for State {
    fn event(_: &mut Self, _: &ZcosmicToplevelInfoV1, _: cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_info_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ExtForeignToplevelListV1, ()> for State {
    fn event(state: &mut Self, _list: &ExtForeignToplevelListV1, event: ext_foreign_toplevel_list_v1::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            state.windows.insert(toplevel.clone(), Win::default());
            if let Some(mgr) = &state.cosmic_mgr {
                mgr.get_cosmic_toplevel(&toplevel, qh, toplevel.clone());
            }
        }
    }

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
    globals.bind::<ExtForeignToplevelListV1, _, _>(qh, 1..=1, ()).ok()?;
    globals.bind::<ZcosmicToplevelInfoV1, _, _>(qh, 2..=3, ()).ok()
}

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

pub fn list_window_titles() -> Option<Vec<String>> {
    let conn = Connection::connect_to_env().ok()?;
    let (globals, mut event_queue) = registry_queue_init::<State>(&conn).ok()?;
    let qh = event_queue.handle();
    let cosmic_mgr = bind_managers(&qh, &globals)?;

    let mut state = State { windows: HashMap::new(), cosmic_mgr: Some(cosmic_mgr), tx: None, last_sent: None };

    event_queue.roundtrip(&mut state).ok()?;
    event_queue.roundtrip(&mut state).ok()?;

    Some(state.windows.values().filter_map(|w| w.title.clone()).filter(|t| !t.is_empty()).collect())
}
