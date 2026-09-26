use image::load_from_memory;
use std::process;
use std::sync::Arc;
use std::thread;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

pub struct Tray {
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    _icon: TrayIcon,
}

fn create_icon() -> Icon {
    let icon = load_from_memory(include_bytes!("../resources/icon.png"))
        .expect("failed to load icon")
        .into_rgba8();
    let (width, height) = icon.dimensions();
    Icon::from_rgba(icon.into_raw(), width, height).expect("failed to create icon")
}

fn build_tray_icon() -> TrayIcon {
    let menu = Menu::new();
    menu.append_items(&[
        &MenuItem::with_id("show", "Show", true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id("quit", "Quit", true, None),
    ])
    .expect("failed to append menu items");

    let builder = TrayIconBuilder::new().with_menu(Box::new(menu)).with_icon(create_icon()).with_tooltip("layerhook");

    #[cfg(target_os = "windows")]
    let builder = builder.with_menu_on_left_click(false);

    builder.build().unwrap()
}

pub fn create_tray_icon(on_show: Arc<dyn Fn() + Send + Sync>) -> Tray {
    thread::spawn({
        let on_show = on_show.clone();
        move || {
            while let Ok(event) = MenuEvent::receiver().recv() {
                match event.id.0.as_str() {
                    "show" => on_show(),
                    "quit" => process::exit(0),
                    _ => {}
                }
            }
        }
    });

    #[cfg(target_os = "windows")]
    thread::spawn(move || {
        use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
        while let Ok(event) = TrayIconEvent::receiver().recv() {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                on_show();
            }
        }
    });

    #[cfg(target_os = "linux")]
    {
        thread::spawn(|| {
            gtk::init().expect("failed to initialize GTK - is a display available?");
            let _icon = build_tray_icon();
            gtk::main();
        });
        Tray {}
    }

    #[cfg(target_os = "windows")]
    {
        thread::spawn(|| {
            let _icon = build_tray_icon();
            pump_windows_messages();
        });
        Tray {}
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    Tray { _icon: build_tray_icon() }
}

#[cfg(target_os = "windows")]
fn pump_windows_messages() {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, TranslateMessage, MSG};

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
