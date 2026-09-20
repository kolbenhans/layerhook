//! Launch layerhook automatically on login. Linux: an XDG autostart
//! `.desktop` file. Windows: a value under the per-user `Run` registry key.
//! Both just point at the current executable's own path — no separate
//! installer/shortcut needed.

#[cfg(target_os = "linux")]
fn desktop_file_path() -> Option<std::path::PathBuf> {
    let dirs = directories::BaseDirs::new()?;
    Some(dirs.config_dir().join("autostart").join("layerhook.desktop"))
}

#[cfg(target_os = "linux")]
pub fn is_enabled() -> bool {
    desktop_file_path().is_some_and(|p| p.exists())
}

#[cfg(target_os = "linux")]
pub fn set_enabled(enabled: bool) {
    let Some(path) = desktop_file_path() else { return };
    if enabled {
        let Ok(exe) = std::env::current_exe() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let contents = format!(
            "[Desktop Entry]\nType=Application\nName=layerhook\nExec={}\nX-GNOME-Autostart-enabled=true\n",
            exe.display()
        );
        let _ = std::fs::write(path, contents);
    } else {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(target_os = "windows")]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(target_os = "windows")]
const RUN_VALUE: &str = "layerhook";

#[cfg(target_os = "windows")]
pub fn is_enabled() -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(RUN_KEY)
        .and_then(|key| key.get_value::<String, _>(RUN_VALUE))
        .is_ok()
}

#[cfg(target_os = "windows")]
pub fn set_enabled(enabled: bool) {
    let Ok(key) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_WRITE) else {
        return;
    };
    if enabled {
        let Ok(exe) = std::env::current_exe() else { return };
        let _ = key.set_value(RUN_VALUE, &exe.display().to_string());
    } else {
        let _ = key.delete_value(RUN_VALUE);
    }
}
