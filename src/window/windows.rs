use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

use windows::core::{PWSTR, BOOL};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumWindows, GetClassNameW, GetForegroundWindow, GetMessageW, GetWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, TranslateMessage,
    EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_SWITCHEND, GW_OWNER, MSG, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
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

static OWNER_NOTE: Mutex<Option<String>> = Mutex::new(None);

pub fn owner_note() -> Option<String> {
    OWNER_NOTE.lock().unwrap().clone()
}

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

fn is_switcher(hwnd: HWND) -> bool {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    matches!(String::from_utf16_lossy(&buf[..n]).as_str(), "XamlExplorerHostIslandWindow" | "MultitaskingViewFrame" | "TaskSwitcherWnd")
}

unsafe extern "system" fn top_window_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = unsafe { &mut *(lparam.0 as *mut Option<HWND>) };
    if unsafe { IsWindowVisible(hwnd) }.as_bool() && window_text(hwnd).is_some() && !is_switcher(hwnd) {
        *out = Some(hwnd);
        return BOOL(0);
    }
    BOOL(1)
}

// ponytail: after Alt+Tab the switcher can keep foreground with no further event; then take the Z-order top
// (EnumWindows is top-first; ignores cloaked windows on other desktops, add DWMWA_CLOAKED check if that bites).
fn effective_foreground() -> HWND {
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_invalid() || !is_switcher(fg) {
        return fg;
    }
    let mut top: Option<HWND> = None;
    unsafe {
        let _ = EnumWindows(Some(top_window_proc), LPARAM(std::ptr::addr_of_mut!(top) as isize));
    }
    top.unwrap_or(fg)
}

static SENDER: OnceLock<Mutex<Sender<Option<String>>>> = OnceLock::new();

unsafe extern "system" fn win_event_proc(_hook: HWINEVENTHOOK, event: u32, hwnd: HWND, _id_object: i32, _id_child: i32, _thread: u32, _time: u32) {
    // ponytail: Alt+Tab may end without a FOREGROUND event for the target window; SWITCHEND re-reads it.
    let hwnd = match event {
        EVENT_SYSTEM_FOREGROUND => hwnd,
        EVENT_SYSTEM_SWITCHEND => effective_foreground(),
        _ => return,
    };
    let Some(sender) = SENDER.get() else { return };
    if hwnd.is_invalid() {
        *OWNER_NOTE.lock().unwrap() = None;
        let _ = sender.lock().unwrap().send(None);
        return;
    }
    let _ = sender.lock().unwrap().send(Some(window_label(hwnd)));
}

pub fn watch(tx: Sender<Option<String>>) {
    let _ = SENDER.set(Mutex::new(tx));
    std::thread::spawn(|| unsafe {
        let hook = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_SWITCHEND, None, Some(win_event_proc), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
        if hook.is_invalid() {
            return;
        }

        let fg = GetForegroundWindow();
        if let Some(sender) = SENDER.get() {
            let _ = sender.lock().unwrap().send(if fg.is_invalid() { None } else { Some(window_label(fg)) });
        }

        // Backstop: the hook alone misses the end of Alt+Tab; main dedupes by layer so repeats are free.
        std::thread::spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let fg = effective_foreground();
            if let Some(sender) = SENDER.get() {
                let _ = sender.lock().unwrap().send(if fg.is_invalid() { None } else { Some(window_label(fg)) });
            }
        });

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnhookWinEvent(hook);
    });
}
