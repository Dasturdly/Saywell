//! Put text into the focused window, either by pasting (save clipboard → set text → Ctrl+V →
//! restore everything) or by typing it as Unicode keystrokes.

use std::time::Duration;

use anyhow::{bail, Result};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    VIRTUAL_KEY, VK_CONTROL, VK_RETURN,
};

use crate::clipboard;

const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Paste,
    Type,
}

impl Method {
    /// From a per-app setting value ("type" or "paste").
    pub fn from_setting(s: &str) -> Self {
        if s.eq_ignore_ascii_case("type") {
            Method::Type
        } else {
            Method::Paste
        }
    }
}

/// Insert `text` with `method`. Returns the method actually used: pasting switches to typing when
/// the clipboard can't be saved (too large or busy), so the user's clipboard is never lost.
pub fn insert(text: &str, method: Method) -> Result<Method> {
    match method {
        Method::Type => type_text(text).map(|_| Method::Type),
        Method::Paste => match clipboard::snapshot() {
            Ok(saved) => paste_with(text, &saved).map(|_| Method::Paste),
            Err(e) => {
                log::warn!("not pasting ({e:#}); typing instead to keep the clipboard");
                type_text(text).map(|_| Method::Type)
            }
        },
    }
}

/// Paste via the clipboard, then restore everything that was on it.
fn paste_with(text: &str, saved: &clipboard::Snapshot) -> Result<()> {
    clipboard::set_text(text, true)?;
    // Clipboard watchers (Windows clipboard history, clipboard managers) open the clipboard right
    // after it changes. If the target app tries to read it at that moment, some apps silently
    // paste nothing. 20 ms failed about 1 in 3 E2E runs; 80 ms passed 12 of 12.
    std::thread::sleep(Duration::from_millis(80));
    let sent = send_ctrl_v();
    // Give the target app time to read the clipboard before restoring it.
    std::thread::sleep(Duration::from_millis(150));
    if let Err(e) = clipboard::restore(saved) {
        log::warn!("could not restore the clipboard: {e:#}");
    }
    sent
}

/// Backwards-compatible helper: paste (or type, if the clipboard can't be saved).
pub fn paste(text: &str) -> Result<()> {
    insert(text, Method::Paste).map(|_| ())
}

/// Put text on the clipboard for the user to paste themselves (e.g. into an admin window).
pub fn copy_for_user(text: &str) -> Result<()> {
    clipboard::set_text(text, false)
}

fn key(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn send(inputs: &[INPUT]) -> Result<()> {
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        bail!("SendInput delivered {sent}/{} events (target window may be elevated)", inputs.len());
    }
    Ok(())
}

fn send_ctrl_v() -> Result<()> {
    let none = KEYBD_EVENT_FLAGS(0);
    send(&[
        key(VK_CONTROL, 0, none),
        key(VK_V, 0, none),
        key(VK_V, 0, KEYEVENTF_KEYUP),
        key(VK_CONTROL, 0, KEYEVENTF_KEYUP),
    ])
}

/// Keystrokes for `text`: each UTF-16 unit as a Unicode key press; newlines as Enter.
fn keystrokes(text: &str) -> Vec<INPUT> {
    let mut inputs = Vec::with_capacity(text.len() * 2);
    for unit in text.replace("\r\n", "\n").encode_utf16() {
        if unit == u16::from(b'\n') {
            inputs.push(key(VK_RETURN, 0, KEYBD_EVENT_FLAGS(0)));
            inputs.push(key(VK_RETURN, 0, KEYEVENTF_KEYUP));
        } else {
            inputs.push(key(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE));
            inputs.push(key(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
        }
    }
    inputs
}

/// Type the text. Sent in small batches so slow apps keep up.
pub fn type_text(text: &str) -> Result<()> {
    for chunk in keystrokes(text).chunks(64) {
        send(chunk)?;
        std::thread::sleep(Duration::from_millis(4));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keystrokes_cover_unicode_and_newlines() {
        let k = keystrokes("Hé\r\n😀");
        // H, é: 2 events each; newline: Enter down/up; emoji: 2 UTF-16 units x 2 events.
        assert_eq!(k.len(), 2 + 2 + 2 + 4);
        let scan = |i: usize| unsafe { k[i].Anonymous.ki.wScan };
        assert_eq!(scan(0), 'H' as u16);
        assert_eq!(scan(2), 'é' as u16);
        assert_eq!(unsafe { k[4].Anonymous.ki.wVk }, VK_RETURN);
        assert_eq!(Method::from_setting("TYPE"), Method::Type);
        assert_eq!(Method::from_setting("paste"), Method::Paste);
    }
}
