//! Windows backend via `SetWinEventHook(EVENT_SYSTEM_FOREGROUND, ...)` -
//! push-based, fires the moment the foreground window changes. Needs a
//! dedicated thread pumping Win32 messages for the hook to actually deliver
//! events (same pattern as the tray icon's hidden host window, see tray.rs).

use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

use windows::core::{PWSTR, BOOL};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumWindows, GetForegroundWindow, GetMessageW, GetWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, TranslateMessage,
    EVENT_SYSTEM_FOREGROUND, GW_OWNER, MSG, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
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

// Display-only: which owner window's title (if any) got substituted for a
// titleless focused window's own (empty) title - see window_text_via_owner()
// below. None when the focused window had its own title directly. Never
// used for rule matching, purely so the GUI status can show it happened;
// hence a plain Mutex read/write rather than routing it through the title
// channel that resolve_layer() actually matches against.
static OWNER_NOTE: Mutex<Option<String>> = Mutex::new(None);

pub fn owner_note() -> Option<String> {
    OWNER_NOTE.lock().unwrap().clone()
}

// Tool palettes (e.g. Photoshop's brush panel) are their own top-level
// window with no title text of their own - the app's main window (which
// does have one) owns them. This alone is no longer load-bearing for rule
// matching (window_label() below adds the exe name, which every window has
// regardless of title), but it still gives a more useful title than "(null)"
// when one's available.
fn window_text_via_owner(hwnd: HWND) -> Option<String> {
    if let Some(text) = window_text(hwnd) {
        *OWNER_NOTE.lock().unwrap() = None;
        return Some(text);
    }
    let mut owner = hwnd;
    for _ in 0..8 {
        owner = unsafe { GetWindow(owner, GW_OWNER) }.ok()?;
        if let Some(text) = window_text(owner) {
            *OWNER_NOTE.lock().unwrap() = Some(text.clone());
            return Some(text);
        }
    }
    *OWNER_NOTE.lock().unwrap() = None;
    None
}

// The exe name alone (e.g. "Photoshop.exe") - stable regardless of which of
// an app's windows has focus, unlike the title (which for something like
// Photoshop's document window changes with zoom/color mode/filename, and is
// flat-out empty for tool palettes). Same idea OBS's window picker uses for
// its "[exe]: title" display.
fn process_exe_name(hwnd: HWND) -> Option<String> {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = vec![0u16; 260];
    let mut len = buf.len() as u32;
    let result = unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) };
    let _ = unsafe { CloseHandle(process) };
    result.ok()?;
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    path.rsplit(['\\', '/']).next().map(str::to_string)
}

// What actually gets matched against (and shown as) the active window:
// "[exe.exe]:window title", same format as OBS's window picker. The exe name
// makes an app identifiable by a simple rule regardless of which of its
// windows has focus - title alone forced increasingly complex regexes for
// apps like Photoshop, whose document window title changes constantly
// (zoom/color mode/filename) and whose tool palettes have no title at all.
fn window_label(hwnd: HWND) -> String {
    let exe = process_exe_name(hwnd).unwrap_or_else(|| "(unknown)".to_string());
    let title = window_text_via_owner(hwnd).unwrap_or_else(|| "(null)".to_string());
    format!("[{exe}]:{title}")
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let titles = unsafe { &mut *(lparam.0 as *mut Vec<String>) };
    if unsafe { IsWindowVisible(hwnd) }.as_bool() {
        if let Some(title) = window_text(hwnd) {
            if !title.is_empty() {
                titles.push(window_label(hwnd));
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
    let Some(sender) = SENDER.get() else { return };
    if hwnd.is_invalid() {
        // Genuinely nothing focused - this is the one case that resets to
        // the default layer.
        *OWNER_NOTE.lock().unwrap() = None;
        let _ = sender.lock().unwrap().send(None);
        return;
    }
    let _ = sender.lock().unwrap().send(Some(window_label(hwnd)));
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
            let _ = sender.lock().unwrap().send(if fg.is_invalid() { None } else { Some(window_label(fg)) });
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
