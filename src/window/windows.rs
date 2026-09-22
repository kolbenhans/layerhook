//! Windows backend via `SetWinEventHook(EVENT_SYSTEM_FOREGROUND, ...)` -
//! push-based, fires the moment the foreground window changes. Needs a
//! dedicated thread pumping Win32 messages for the hook to actually deliver
//! events (same pattern as the tray icon's hidden host window, see tray.rs).

use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumWindows, GetForegroundWindow, GetMessageW, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible, TranslateMessage, EVENT_SYSTEM_FOREGROUND, MSG,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

fn window_text(hwnd: HWND) -> Option<String> {
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u16; (len + 1) as usize];
    let copied = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if copied == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..copied as usize]))
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let titles = unsafe { &mut *(lparam.0 as *mut Vec<String>) };
    if unsafe { IsWindowVisible(hwnd) }.as_bool() {
        if let Some(title) = window_text(hwnd) {
            if !title.is_empty() {
                titles.push(title);
            }
        }
    }
    BOOL(1)
}

pub fn list_window_titles() -> Vec<String> {
    let mut titles: Vec<String> = Vec::new();
    let lparam = LPARAM(std::ptr::addr_of_mut!(titles) as isize);
    unsafe {
        let _ = EnumWindows(Some(enum_proc), lparam);
    }
    titles
}

// Only one watcher is ever started (once, at app startup) - a static is
// simpler than threading a channel through the WINEVENTPROC's fixed C
// callback signature, which can't capture a closure.
static SENDER: OnceLock<Mutex<Sender<Option<String>>>> = OnceLock::new();

unsafe extern "system" fn win_event_proc(_hook: HWINEVENTHOOK, event: u32, hwnd: HWND, _id_object: i32, _id_child: i32, _thread: u32, _time: u32) {
    if event != EVENT_SYSTEM_FOREGROUND {
        return;
    }
    let title = if hwnd.is_invalid() { None } else { window_text(hwnd) };
    if let Some(sender) = SENDER.get() {
        let _ = sender.lock().unwrap().send(title);
    }
}

pub fn watch(tx: Sender<Option<String>>) {
    let _ = SENDER.set(Mutex::new(tx));
    std::thread::spawn(|| unsafe {
        let hook = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(win_event_proc), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
        if hook.is_invalid() {
            return;
        }

        // Seed with whatever's focused right now - the hook only fires on
        // the *next* change.
        let fg = GetForegroundWindow();
        if let Some(sender) = SENDER.get() {
            let _ = sender.lock().unwrap().send(if fg.is_invalid() { None } else { window_text(fg) });
        }

        // The hook only delivers events while this thread pumps messages.
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnhookWinEvent(hook);
    });
}
