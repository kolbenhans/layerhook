use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible,
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

pub fn active_window_title() -> Option<String> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    window_text(hwnd)
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
