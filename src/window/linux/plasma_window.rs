use std::collections::HashMap;
use std::sync::mpsc::Sender;

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_plasma::plasma_window_management::client::{
    org_kde_plasma_window::{self, OrgKdePlasmaWindow},
    org_kde_plasma_window_management::{self, OrgKdePlasmaWindowManagement},
};

const STATE_ACTIVE: u32 = 0x1;

#[derive(Default)]
struct Win {
    title: Option<String>,
    active: bool,
}

struct State {
    windows: HashMap<OrgKdePlasmaWindow, Win>,
    tx: Option<Sender<Option<String>>>,
    last_sent: Option<String>,
}

impl State {
    fn active_title(&self) -> Option<String> {
        self.windows.values().find(|w| w.active).and_then(|w| w.title.clone())
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

impl Dispatch<OrgKdePlasmaWindowManagement, ()> for State {
    fn event(state: &mut Self, mgr: &OrgKdePlasmaWindowManagement, event: org_kde_plasma_window_management::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        use org_kde_plasma_window_management::Event;
        match event {
            Event::Window { id } => {
                let handle = mgr.get_window(id, qh, ());
                state.windows.insert(handle, Win::default());
            }
            Event::WindowWithUuid { uuid, .. } => {
                let handle = mgr.get_window_by_uuid(uuid, qh, ());
                state.windows.insert(handle, Win::default());
            }
            _ => {}
        }
    }
}

impl Dispatch<OrgKdePlasmaWindow, ()> for State {
    fn event(state: &mut Self, handle: &OrgKdePlasmaWindow, event: org_kde_plasma_window::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use org_kde_plasma_window::Event;
        match event {
            Event::TitleChanged { title } => {
                state.windows.entry(handle.clone()).or_default().title = Some(title);
                state.maybe_notify();
            }
            Event::StateChanged { flags } => {
                state.windows.entry(handle.clone()).or_default().active = flags & STATE_ACTIVE != 0;
                state.maybe_notify();
            }
            Event::Unmapped => {
                state.windows.remove(handle);
                state.maybe_notify();
            }
            _ => {}
        }
    }
}

fn bind_manager(qh: &QueueHandle<State>, globals: &wayland_client::globals::GlobalList) -> Option<OrgKdePlasmaWindowManagement> {
    globals.bind::<OrgKdePlasmaWindowManagement, _, _>(qh, 1..=18, ()).ok()
}

pub fn watch(tx: Sender<Option<String>>) -> bool {
    let Ok(conn) = Connection::connect_to_env() else { return false };
    let Ok((globals, mut event_queue)) = registry_queue_init::<State>(&conn) else { return false };
    let qh = event_queue.handle();
    let Some(_manager) = bind_manager(&qh, &globals) else { return false };

    let mut state = State { windows: HashMap::new(), tx: Some(tx), last_sent: None };
    std::thread::spawn(move || {
        while event_queue.blocking_dispatch(&mut state).is_ok() {}
    });
    true
}

pub fn list_window_titles() -> Option<Vec<String>> {
    let conn = Connection::connect_to_env().ok()?;
    let (globals, mut event_queue) = registry_queue_init::<State>(&conn).ok()?;
    let qh = event_queue.handle();
    let _manager = bind_manager(&qh, &globals)?;

    let mut state = State { windows: HashMap::new(), tx: None, last_sent: None };

    event_queue.roundtrip(&mut state).ok()?;
    event_queue.roundtrip(&mut state).ok()?;

    Some(state.windows.values().filter_map(|w| w.title.clone()).filter(|t| !t.is_empty()).collect())
}
