//! Global push-to-talk via a Win32 low-level keyboard hook.
//!
//! A low-level hook sees real key-down/key-up events (unlike RegisterHotKey), and works with a
//! lone modifier such as Right Ctrl. The hook never swallows keys, so the keyboard keeps working
//! normally even if Saywell misbehaves.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;

use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage, HHOOK, KBDLLHOOKSTRUCT, MSG,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

const VK_ESCAPE: u32 = 0x1B;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed,
    Released,
    /// Esc pressed while the hotkey is held.
    Cancel,
}

static SENDER: OnceLock<Sender<HotkeyEvent>> = OnceLock::new();
static HOTKEY_VK: AtomicU32 = AtomicU32::new(0xA3);
static HELD: AtomicBool = AtomicBool::new(false);
/// While true, the hotkey is ignored (the settings window is capturing a new key).
static PAUSED: AtomicBool = AtomicBool::new(false);
/// While true, Esc cancels even though the hotkey is not held (hands-free recording).
static ESC_ARMED: AtomicBool = AtomicBool::new(false);

/// Key events reported by Saywell's own windows. While one of them has focus, the embedded browser
/// engine can take keyboard input before the low-level hook sees it, so the pages forward the
/// dictation key (and Esc) themselves. `HELD` de-duplicates if both paths report the same press.
pub fn page_key(vk: u32, down: bool) {
    if let Some(ev) = classify(vk, if down { WM_KEYDOWN } else { WM_KEYUP }) {
        log::debug!("hotkey {ev:?} (from page)");
        if let Some(tx) = SENDER.get() {
            let _ = tx.send(ev);
        }
    }
}

/// Change the hotkey while running.
pub fn set_vk(vk: u32) {
    HOTKEY_VK.store(vk, Ordering::SeqCst);
    HELD.store(false, Ordering::SeqCst);
}

pub fn set_paused(paused: bool) {
    PAUSED.store(paused, Ordering::SeqCst);
    HELD.store(false, Ordering::SeqCst);
}

pub fn set_esc_armed(armed: bool) {
    ESC_ARMED.store(armed, Ordering::SeqCst);
}

/// Install the hook on its own thread (it needs a message loop). Call once.
pub fn spawn(vk: u32, tx: Sender<HotkeyEvent>) {
    HOTKEY_VK.store(vk, Ordering::SeqCst);
    if SENDER.set(tx).is_err() {
        log::warn!("hotkey hook already installed");
        return;
    }
    std::thread::Builder::new()
        .name("hotkey".into())
        .spawn(|| unsafe {
            let hmod = GetModuleHandleW(None).map(HINSTANCE::from).unwrap_or_default();
            match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), hmod, 0) {
                Ok(_) => log::info!("keyboard hook installed (vk=0x{:02X})", HOTKEY_VK.load(Ordering::SeqCst)),
                Err(e) => {
                    log::error!("failed to install keyboard hook: {e}");
                    return;
                }
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .expect("spawn hotkey thread");
}

/// Map a raw key event to a hotkey event, updating the held state. Pure apart from `HELD`.
fn classify(vk: u32, msg: u32) -> Option<HotkeyEvent> {
    let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
    if PAUSED.load(Ordering::Relaxed) {
        return None;
    }
    if vk == HOTKEY_VK.load(Ordering::Relaxed) {
        if down && !HELD.swap(true, Ordering::SeqCst) {
            return Some(HotkeyEvent::Pressed); // first down only; ignore auto-repeat
        }
        if up && HELD.swap(false, Ordering::SeqCst) {
            return Some(HotkeyEvent::Released);
        }
    } else if vk == VK_ESCAPE && down && (HELD.load(Ordering::SeqCst) || ESC_ARMED.load(Ordering::SeqCst)) {
        return Some(HotkeyEvent::Cancel);
    }
    None
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        if let Some(ev) = classify(kb.vkCode, wparam.0 as u32) {
            log::debug!("hotkey {ev:?}");
            if let Some(tx) = SENDER.get() {
                let _ = tx.send(ev);
            }
        }
    }
    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_repeat_release_cancel() {
        HOTKEY_VK.store(0xA3, Ordering::SeqCst);
        HELD.store(false, Ordering::SeqCst);
        assert_eq!(classify(0xA3, WM_KEYDOWN), Some(HotkeyEvent::Pressed));
        assert_eq!(classify(0xA3, WM_KEYDOWN), None, "auto-repeat ignored");
        assert_eq!(classify(VK_ESCAPE, WM_KEYDOWN), Some(HotkeyEvent::Cancel));
        assert_eq!(classify(0x41, WM_KEYDOWN), None, "other keys ignored");
        assert_eq!(classify(0xA3, WM_KEYUP), Some(HotkeyEvent::Released));
        assert_eq!(classify(0xA3, WM_KEYUP), None);
        assert_eq!(classify(VK_ESCAPE, WM_KEYDOWN), None, "Esc ignored when not held");
        set_esc_armed(true);
        assert_eq!(classify(VK_ESCAPE, WM_KEYDOWN), Some(HotkeyEvent::Cancel), "Esc cancels hands-free");
        set_esc_armed(false);
        set_paused(true);
        assert_eq!(classify(0xA3, WM_KEYDOWN), None, "paused while capturing a new key");
        set_paused(false);
        set_vk(0xA5);
        assert_eq!(classify(0xA3, WM_KEYDOWN), None);
        assert_eq!(classify(0xA5, WM_KEYDOWN), Some(HotkeyEvent::Pressed));
        assert_eq!(classify(0xA5, WM_KEYUP), Some(HotkeyEvent::Released));
    }
}
