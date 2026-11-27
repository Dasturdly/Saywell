//! Small Windows helpers: single-instance guard, start-with-Windows, and no-activate windows.

use anyhow::{bail, Result};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, HWND};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, MessageBoxW, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, HWND_TOPMOST,
    MB_ICONINFORMATION, MB_OK, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNOACTIVATE, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW,
};

/// Returns false if another Saywell is already running in this Windows session.
pub fn acquire_single_instance() -> bool {
    unsafe {
        match CreateMutexW(None, true, &HSTRING::from("Local\\Saywell-SingleInstance-7f3c")) {
            // The handle is intentionally leaked: the mutex lives as long as this process.
            Ok(_) => GetLastError() != ERROR_ALREADY_EXISTS,
            Err(_) => true, // can't tell; don't block startup
        }
    }
}

pub fn message_box(title: &str, text: &str) {
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(title), MB_OK | MB_ICONINFORMATION);
    }
}

/// True if the foreground window belongs to Saywell itself (settings or setup window).
pub fn foreground_is_ours() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
        pid == std::process::id()
    }
}

/// Make a window never take focus and stay out of Alt+Tab.
pub fn make_no_activate(hwnd: isize) {
    unsafe {
        let h = HWND(hwnd as _);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE.0 as isize | WS_EX_TOOLWINDOW.0 as isize);
    }
}

/// Show a window on top without activating it (so focus stays in the app being dictated into).
pub fn show_no_activate(hwnd: isize) {
    unsafe {
        let h = HWND(hwnd as _);
        let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

pub const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
pub const RUN_VALUE: &str = "Saywell";

fn open_key(subkey: &str) -> Result<HKEY> {
    let mut key = HKEY::default();
    let err = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(subkey),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            None,
            &mut key,
            None,
        )
    };
    if err.is_err() {
        bail!("opening HKCU\\{subkey}: {err:?}");
    }
    Ok(key)
}

/// Read a string value under HKCU\`subkey`; `None` if it does not exist.
pub fn get_string(subkey: &str, name: &str) -> Result<Option<String>> {
    let key = open_key(subkey)?;
    let mut buf = vec![0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let err = unsafe {
        RegQueryValueExW(key, &HSTRING::from(name), None, None, Some(buf.as_mut_ptr() as *mut u8), Some(&mut len))
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    if err == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if err.is_err() {
        bail!("reading {name}: {err:?}");
    }
    let chars = (len as usize / 2).min(buf.len());
    Ok(Some(String::from_utf16_lossy(&buf[..chars]).trim_end_matches('\0').to_string()))
}

/// Set (Some) or delete (None) a string value under HKCU\`subkey`.
pub fn set_string(subkey: &str, name: &str, value: Option<&str>) -> Result<()> {
    let key = open_key(subkey)?;
    let err = unsafe {
        match value {
            Some(v) => {
                let wide: Vec<u16> = v.encode_utf16().chain(std::iter::once(0)).collect();
                let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
                RegSetValueExW(key, &HSTRING::from(name), 0, REG_SZ, Some(bytes))
            }
            None => match RegDeleteValueW(key, &HSTRING::from(name)) {
                e if e == ERROR_FILE_NOT_FOUND => Default::default(),
                e => e,
            },
        }
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    if err.is_err() {
        bail!("writing {name}: {err:?}");
    }
    Ok(())
}

/// Is Saywell registered to start with Windows (pointing at this exe)?
pub fn autostart_enabled() -> bool {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    matches!(get_string(RUN_KEY, RUN_VALUE), Ok(Some(v)) if v.contains(&exe))
}

pub fn set_autostart(on: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    let cmd = format!("\"{}\"", exe.display());
    set_string(RUN_KEY, RUN_VALUE, on.then_some(cmd.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_round_trip_in_scratch_key() {
        let key = format!("Software\\Saywell-Test-{}", std::process::id());
        set_string(&key, "v", Some("\"C:\\a b\\saywell.exe\"")).unwrap();
        assert_eq!(get_string(&key, "v").unwrap().as_deref(), Some("\"C:\\a b\\saywell.exe\""));
        set_string(&key, "v", None).unwrap();
        assert_eq!(get_string(&key, "v").unwrap(), None);
        set_string(&key, "v", None).unwrap(); // deleting twice is fine
        unsafe {
            let _ = windows::Win32::System::Registry::RegDeleteKeyW(HKEY_CURRENT_USER, &HSTRING::from(key.as_str()));
        }
    }

    #[test]
    fn second_instance_is_detected() {
        // Same name as the app uses, but scoped to this test via a different suffix.
        unsafe {
            let name = HSTRING::from(format!("Local\\Saywell-Test-{}", std::process::id()));
            let _first = CreateMutexW(None, true, &name).unwrap();
            assert_ne!(GetLastError(), ERROR_ALREADY_EXISTS);
            let _second = CreateMutexW(None, true, &name).unwrap();
            assert_eq!(GetLastError(), ERROR_ALREADY_EXISTS);
        }
    }
}
