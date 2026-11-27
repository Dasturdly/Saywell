//! Put text into the focused window: save clipboard → set text → Ctrl+V → restore clipboard.

use std::time::Duration;

use anyhow::{bail, Result};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL,
};

const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);

/// Paste `text` into whatever window has keyboard focus, then restore the previous clipboard text.
pub fn paste(text: &str) -> Result<()> {
    let mut cb = arboard::Clipboard::new()?;
    let previous = cb.get_text().ok(); // non-text clipboard contents are not preserved yet (M5)
    cb.set_text(text.to_owned())?;
    std::thread::sleep(Duration::from_millis(20));
    send_ctrl_v()?;
    // Give the target app time to read the clipboard before restoring it.
    std::thread::sleep(Duration::from_millis(150));
    match previous {
        Some(p) => cb.set_text(p)?,
        None => cb.clear()?,
    }
    Ok(())
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_ctrl_v() -> Result<()> {
    let inputs = [key(VK_CONTROL, false), key(VK_V, false), key(VK_V, true), key(VK_CONTROL, true)];
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        bail!("SendInput delivered {sent}/{} events (target window may be elevated)", inputs.len());
    }
    Ok(())
}
