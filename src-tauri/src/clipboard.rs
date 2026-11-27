//! Clipboard access via Win32: keep *everything* that was on the clipboard (images, files, rich
//! text), and mark Saywell's temporary text so Windows clipboard history and cloud clipboard
//! don't record it.

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE};

const CF_TEXT: u32 = 1;
const CF_BITMAP: u32 = 2;
const CF_METAFILEPICT: u32 = 3;
const CF_OEMTEXT: u32 = 7;
const CF_PALETTE: u32 = 9;
const CF_UNICODETEXT: u32 = 13;
const CF_ENHMETAFILE: u32 = 14;
const CF_DIBV5: u32 = 17;
const CF_DIB: u32 = 8;
/// Formats whose data is a GDI handle, not memory; Windows re-creates them from the memory formats.
const HANDLE_FORMATS: [u32; 4] = [CF_BITMAP, CF_METAFILEPICT, CF_PALETTE, CF_ENHMETAFILE];
/// Don't snapshot clipboards larger than this (e.g. a huge spreadsheet range); only text is kept then.
const MAX_SNAPSHOT_BYTES: usize = 64 << 20;

/// What was on the clipboard: each format's raw bytes.
pub struct Snapshot {
    formats: Vec<(u32, Vec<u8>)>,
}

impl Snapshot {
    pub fn is_empty(&self) -> bool {
        self.formats.is_empty()
    }
    pub fn len(&self) -> usize {
        self.formats.len()
    }
}

/// Holds the clipboard open; closes it and destroys the owner window on drop.
struct Open {
    hwnd: HWND,
}

impl Open {
    /// Open the clipboard with a private owner window, retrying while another program holds it.
    fn new() -> Result<Self> {
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                None,
                None,
                None,
            )?
        };
        let deadline = Instant::now() + Duration::from_millis(600);
        loop {
            if unsafe { OpenClipboard(hwnd) }.is_ok() {
                return Ok(Self { hwnd });
            }
            if Instant::now() > deadline {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                bail!("the clipboard is busy (another program has it open)");
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn format_id(name: &str) -> u32 {
    unsafe { RegisterClipboardFormatW(&HSTRING::from(name)) }
}

/// Copy the bytes of the clipboard data for `fmt`, if it is memory-based.
unsafe fn read_format(fmt: u32) -> Option<Vec<u8>> {
    let h = GetClipboardData(fmt).ok()?;
    if h.is_invalid() {
        return None;
    }
    let hg = HGLOBAL(h.0);
    let size = GlobalSize(hg);
    let p = GlobalLock(hg) as *const u8;
    if p.is_null() {
        return None;
    }
    let bytes = std::slice::from_raw_parts(p, size).to_vec();
    let _ = GlobalUnlock(hg);
    Some(bytes)
}

unsafe fn write_format(fmt: u32, bytes: &[u8]) -> Result<()> {
    let hg = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
    let p = GlobalLock(hg) as *mut u8;
    if p.is_null() {
        bail!("GlobalLock failed");
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
    let _ = GlobalUnlock(hg);
    // On success the system owns the memory.
    SetClipboardData(fmt, HANDLE(hg.0)).map_err(|e| anyhow!("SetClipboardData({fmt}): {e}"))?;
    Ok(())
}

/// Tell Windows clipboard history, cloud clipboard and clipboard managers to ignore this content.
unsafe fn mark_private() {
    let zero = 0u32.to_le_bytes();
    let _ = write_format(format_id("ExcludeClipboardContentFromMonitorProcessing"), &zero);
    let _ = write_format(format_id("CanIncludeInClipboardHistory"), &zero);
    let _ = write_format(format_id("CanUploadToCloudClipboard"), &zero);
}

fn private_formats() -> [u32; 3] {
    [
        format_id("ExcludeClipboardContentFromMonitorProcessing"),
        format_id("CanIncludeInClipboardHistory"),
        format_id("CanUploadToCloudClipboard"),
    ]
}

/// Save everything currently on the clipboard.
pub fn snapshot() -> Result<Snapshot> {
    let _open = Open::new()?;
    let skip = private_formats();
    let mut formats = Vec::new();
    let mut total = 0usize;
    let mut has_unicode = false;
    let mut has_dib = false;
    let mut fmt = 0u32;
    loop {
        fmt = unsafe { EnumClipboardFormats(fmt) };
        if fmt == 0 {
            break;
        }
        has_unicode |= fmt == CF_UNICODETEXT;
        has_dib |= fmt == CF_DIB;
        if HANDLE_FORMATS.contains(&fmt) || skip.contains(&fmt) {
            continue;
        }
        if let Some(bytes) = unsafe { read_format(fmt) } {
            total += bytes.len();
            if total > MAX_SNAPSHOT_BYTES {
                bail!("clipboard too large to save ({} MB)", total >> 20);
            }
            formats.push((fmt, bytes));
        }
    }
    // Windows synthesizes these from the Unicode text / DIB; restoring them too can pick the wrong one.
    formats.retain(|(f, _)| !(has_unicode && (*f == CF_TEXT || *f == CF_OEMTEXT)) && !(has_dib && *f == CF_DIBV5));
    Ok(Snapshot { formats })
}

/// Put back a snapshot. Marked private so clipboard history doesn't record it a second time.
pub fn restore(s: &Snapshot) -> Result<()> {
    let _open = Open::new()?;
    unsafe {
        EmptyClipboard()?;
        for (fmt, bytes) in &s.formats {
            if let Err(e) = write_format(*fmt, bytes) {
                log::warn!("could not restore clipboard format {fmt}: {e:#}");
            }
        }
        if !s.formats.is_empty() {
            mark_private();
        }
    }
    Ok(())
}

fn utf16z(text: &str) -> Vec<u8> {
    text.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect()
}

/// Put `text` on the clipboard. `private` hides it from clipboard history and cloud clipboard
/// (used for Saywell's temporary paste; not for text the user is meant to paste themselves).
pub fn set_text(text: &str, private: bool) -> Result<()> {
    let _open = Open::new()?;
    unsafe {
        EmptyClipboard()?;
        write_format(CF_UNICODETEXT, &utf16z(text))?;
        if private {
            mark_private();
        }
    }
    Ok(())
}

/// Current clipboard text, if any.
pub fn get_text() -> Option<String> {
    let _open = Open::new().ok()?;
    let bytes = unsafe { read_format(CF_UNICODETEXT)? };
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    Some(String::from_utf16_lossy(&units[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uses the real clipboard, so it saves and restores whatever the user had on it.
    #[test]
    #[ignore]
    fn round_trips_custom_formats_and_marks_private_text() {
        let users = snapshot().expect("snapshot user's clipboard");
        let custom = format_id("Saywell-Test-Format");
        {
            let _o = Open::new().unwrap();
            unsafe {
                EmptyClipboard().unwrap();
                write_format(CF_UNICODETEXT, &utf16z("original text")).unwrap();
                write_format(custom, &[1, 2, 3, 250]).unwrap();
            }
        }
        let saved = snapshot().unwrap();
        set_text("dictated", true).unwrap();
        assert_eq!(get_text().as_deref(), Some("dictated"));
        {
            let _o = Open::new().unwrap();
            let fmts: Vec<u32> = std::iter::successors(Some(0u32), |&f| {
                let n = unsafe { EnumClipboardFormats(f) };
                (n != 0).then_some(n)
            })
            .skip(1)
            .collect();
            assert!(private_formats().iter().all(|f| fmts.contains(f)), "private markers present");
        }
        restore(&saved).unwrap();
        assert_eq!(get_text().as_deref(), Some("original text"));
        let again = snapshot().unwrap();
        assert!(again.formats.iter().any(|(f, b)| *f == custom && b[..4] == [1, 2, 3, 250]), "custom format restored");
        restore(&users).expect("restore user's clipboard");
    }
}
