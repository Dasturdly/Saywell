//! Windows (overlay pill, settings, first-run setup) and the commands their pages call.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

use crate::store::{HistoryEntry, Rule, Term};
use crate::{audio, hotkey, insert, models, settings, settings_snapshot, winsys, SETTINGS, STORE};

const OVERLAY_W: f64 = 480.0;
const OVERLAY_H: f64 = 116.0;
/// Bumped on every overlay change, so a delayed "hide" never hides a newer state.
static OVERLAY_GEN: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Serialize)]
struct OverlayMsg<'a> {
    state: &'a str,
    text: &'a str,
}

/// Create the overlay window once, hidden. It never takes focus and ignores the mouse.
pub fn create_overlay(app: &AppHandle) -> tauri::Result<()> {
    let win = WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("overlay.html".into()))
        .title("Saywell")
        .inner_size(OVERLAY_W, OVERLAY_H)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .focusable(false)
        .visible(false)
        .build()?;
    win.set_ignore_cursor_events(true)?;
    if let Ok(h) = win.hwnd() {
        winsys::make_no_activate(h.0 as isize);
    }
    Ok(())
}

/// Bottom-centre of the work area of the monitor under the mouse pointer.
fn place_overlay(app: &AppHandle, win: &tauri::WebviewWindow) {
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    if let Some(m) = monitor {
        let scale = m.scale_factor();
        let area = m.work_area();
        let (w, h) = ((OVERLAY_W * scale) as i32, (OVERLAY_H * scale) as i32);
        let x = area.position.x + (area.size.width as i32 - w) / 2;
        let y = area.position.y + area.size.height as i32 - h - (24.0 * scale) as i32;
        let _ = win.set_size(LogicalSize::new(OVERLAY_W, OVERLAY_H));
        let _ = win.set_position(PhysicalPosition::new(x, y));
        log::debug!("overlay at {x},{y} size {w}x{h}");
    }
}

/// Show the overlay in `state` ("listening", "handsfree", "thinking", "error") or hide it ("hidden").
pub fn overlay(app: &AppHandle, state: &str, text: &str) {
    OVERLAY_GEN.fetch_add(1, Ordering::SeqCst);
    let Some(win) = app.get_webview_window("overlay") else { return };
    log::debug!("overlay -> {state}");
    let _ = app.emit_to("overlay", "overlay", OverlayMsg { state, text });
    if state == "hidden" {
        let _ = win.hide();
    } else if !win.is_visible().unwrap_or(false) {
        place_overlay(app, &win);
        if let Ok(h) = win.hwnd() {
            winsys::show_no_activate(h.0 as isize);
        }
    }
}

pub fn overlay_level(app: &AppHandle, level: f32) {
    let _ = app.emit_to("overlay", "overlay-level", level);
}

pub fn overlay_preview(app: &AppHandle, text: &str) {
    let _ = app.emit_to("overlay", "overlay-preview", text);
}

/// Show a short message, then hide (unless something newer was shown meanwhile).
pub fn overlay_error(app: &AppHandle, msg: &str) {
    overlay(app, "error", msg);
    let gen = OVERLAY_GEN.load(Ordering::SeqCst);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(2500));
        if OVERLAY_GEN.load(Ordering::SeqCst) == gen {
            overlay(&app, "hidden", "");
        }
    });
}

fn open_window(app: &AppHandle, label: &str, page: &str, title: &str, w: f64, h: f64) {
    if let Some(win) = app.get_webview_window(label) {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    let res = WebviewWindowBuilder::new(app, label, WebviewUrl::App(page.into()))
        .title(title)
        .inner_size(w, h)
        .min_inner_size(w * 0.8, h * 0.8)
        .center()
        .build();
    if let Err(e) = res {
        log::error!("could not open {label} window: {e}");
    }
}

pub fn open_settings(app: &AppHandle) {
    open_window(app, "settings", "settings.html", "Saywell settings", 560.0, 640.0);
}

pub fn open_history_window(app: &AppHandle) {
    open_window(app, "history", "history.html", "Saywell history", 640.0, 680.0);
}

pub fn open_onboarding(app: &AppHandle) {
    open_window(app, "onboarding", "onboarding.html", "Welcome to Saywell", 620.0, 600.0);
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct SettingsView {
    hotkey_vk: u32,
    hotkey_label: String,
    double_tap_hands_free: bool,
    preview_caption: bool,
    cleanup_enabled: bool,
    mic_device: Option<String>,
    start_with_windows: bool,
    history_enabled: bool,
    history_days: u32,
    mics: Vec<String>,
    models_dir: String,
    version: &'static str,
}

#[tauri::command]
pub fn get_settings() -> SettingsView {
    log::debug!("command get_settings");
    let s = settings_snapshot();
    SettingsView {
        hotkey_vk: s.hotkey_vk,
        hotkey_label: s.hotkey_label,
        double_tap_hands_free: s.double_tap_hands_free,
        preview_caption: s.preview_caption,
        cleanup_enabled: s.cleanup_enabled,
        mic_device: s.mic_device,
        start_with_windows: winsys::autostart_enabled(),
        history_enabled: s.history_enabled,
        history_days: s.history_days,
        mics: audio::input_devices(),
        models_dir: s.models_dir.display().to_string(),
        version: env!("CARGO_PKG_VERSION"),
    }
}

#[derive(Deserialize)]
pub struct SettingsPatch {
    hotkey_vk: Option<u32>,
    hotkey_label: Option<String>,
    double_tap_hands_free: Option<bool>,
    preview_caption: Option<bool>,
    cleanup_enabled: Option<bool>,
    /// `Some("")` means the Windows default.
    mic_device: Option<String>,
    start_with_windows: Option<bool>,
    history_enabled: Option<bool>,
    history_days: Option<u32>,
}

/// Apply and save the given changes; returns the new settings.
#[tauri::command]
pub fn update_settings(app: AppHandle, patch: SettingsPatch) -> Result<SettingsView, String> {
    if let Some(on) = patch.start_with_windows {
        winsys::set_autostart(on).map_err(|e| format!("Couldn't change start-up setting: {e:#}"))?;
    }
    if let Some(on) = patch.cleanup_enabled {
        crate::set_cleanup(&app, on); // saves too
    }
    let guard = SETTINGS.get().ok_or("settings not loaded")?;
    {
        let mut s = guard.lock().unwrap();
        if let (Some(vk), Some(label)) = (patch.hotkey_vk, patch.hotkey_label) {
            if !(1..=0xFE).contains(&vk) || vk == 0x1B {
                return Err("That key can't be used.".into());
            }
            s.hotkey_vk = vk;
            s.hotkey_label = label;
            hotkey::set_vk(vk);
        }
        if let Some(v) = patch.double_tap_hands_free {
            s.double_tap_hands_free = v;
        }
        if let Some(v) = patch.preview_caption {
            s.preview_caption = v;
        }
        if let Some(m) = patch.mic_device {
            s.mic_device = (!m.is_empty()).then_some(m);
        }
        if let Some(v) = patch.history_enabled {
            s.history_enabled = v;
        }
        if let Some(d) = patch.history_days {
            s.history_days = d;
            if let Some(store) = STORE.get() {
                let _ = store.prune_history(d);
            }
        }
        s.save(&settings::settings_path()).map_err(|e| format!("Couldn't save settings: {e:#}"))?;
    }
    crate::refresh_status(&app);
    if patch.history_days.is_some() {
        crate::history_changed(&app);
    }
    Ok(get_settings())
}

/// While the settings page waits for a new hotkey, dictation is paused.
#[tauri::command]
pub fn capture_hotkey(active: bool) {
    hotkey::set_paused(active);
}

#[derive(Serialize)]
pub struct ModelStatus {
    #[serde(flatten)]
    spec: models::ModelSpec,
    present: bool,
}

#[derive(Serialize)]
pub struct SetupStatus {
    models: Vec<ModelStatus>,
    models_dir: String,
    cleanup_enabled: bool,
    hotkey_label: String,
    engines_started: bool,
}

#[tauri::command]
pub fn setup_status() -> SetupStatus {
    log::debug!("command setup_status");
    let s = settings_snapshot();
    let models = models::catalogue()
        .into_iter()
        .map(|spec| {
            let path = if spec.id == "speech" { s.whisper_model_path() } else { s.llm_model_path() };
            ModelStatus { spec, present: path.exists() }
        })
        .collect();
    SetupStatus {
        models,
        models_dir: s.models_dir.display().to_string(),
        cleanup_enabled: s.cleanup_enabled,
        hotkey_label: s.hotkey_label,
        engines_started: crate::ENGINES_STARTED.load(Ordering::SeqCst),
    }
}

static DOWNLOAD_CANCEL: AtomicBool = AtomicBool::new(false);
static DOWNLOADING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize)]
struct DownloadEvent {
    id: String,
    state: &'static str, // "progress" | "done" | "error"
    done: u64,
    total: u64,
    error: Option<String>,
}

/// Download the given models one after another, reporting progress as "download" events.
#[tauri::command]
pub fn start_download(app: AppHandle, ids: Vec<String>) -> Result<(), String> {
    if DOWNLOADING.swap(true, Ordering::SeqCst) {
        return Err("A download is already running.".into());
    }
    DOWNLOAD_CANCEL.store(false, Ordering::SeqCst);
    let dir = settings_snapshot().models_dir;
    // For offline installs or testing: fetch from a local mirror instead of Hugging Face.
    let mirror = std::env::var("SAYWELL_MODEL_MIRROR").ok().filter(|m| !m.is_empty());
    std::thread::spawn(move || {
        for id in ids {
            let Some(spec) = models::by_id(&id) else { continue };
            let url = match &mirror {
                Some(m) => format!("{}/{}", m.trim_end_matches('/'), spec.file),
                None => spec.url.to_string(),
            };
            log::info!("downloading {} from {}", spec.file, url);
            let mut last_emit = std::time::Instant::now() - Duration::from_secs(1);
            let res = models::download(&spec, &url, &dir, &DOWNLOAD_CANCEL, |done, total| {
                if last_emit.elapsed() >= Duration::from_millis(200) || done == total {
                    last_emit = std::time::Instant::now();
                    let ev = DownloadEvent { id: id.clone(), state: "progress", done, total, error: None };
                    let _ = app.emit("download", ev);
                }
            });
            let ev = match res {
                Ok(_) => {
                    log::info!("{} downloaded and verified", spec.file);
                    DownloadEvent { id: id.clone(), state: "done", done: spec.size, total: spec.size, error: None }
                }
                Err(e) => {
                    log::error!("download of {} failed: {e:#}", spec.file);
                    let error = Some(format!("{e:#}"));
                    let _ = app.emit("download", DownloadEvent { id, state: "error", done: 0, total: spec.size, error });
                    break;
                }
            };
            let _ = app.emit("download", ev);
        }
        DOWNLOADING.store(false, Ordering::SeqCst);
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_download() {
    DOWNLOAD_CANCEL.store(true, Ordering::SeqCst);
}

#[tauri::command]
pub fn open_models_dir() -> Result<(), String> {
    let dir = settings_snapshot().models_dir;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::process::Command::new("explorer").arg(&dir).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

/// Called when setup has the models it needs: saves the cleanup choice and starts the engines.
#[tauri::command]
pub fn finish_setup(app: AppHandle, cleanup: bool) -> Result<(), String> {
    let s = settings_snapshot();
    if !s.whisper_model_path().exists() {
        return Err("The speech model is still missing.".into());
    }
    crate::set_cleanup(&app, cleanup && s.llm_model_path().exists());
    crate::start_engines(&app);
    Ok(())
}

#[tauri::command]
pub fn close_setup(app: AppHandle) {
    if let Some(w) = app.get_webview_window("onboarding") {
        let _ = w.close();
    }
}

/// Hotkey presses seen by a Saywell page (see `hotkey::page_key`).
#[tauri::command]
pub fn page_key(vk: u32, down: bool) {
    hotkey::page_key(vk, down);
}

/// Deliver dictated text to Saywell's own focused window (its page inserts it at the cursor).
pub fn insert_into_own_window(app: &AppHandle, text: &str) {
    let _ = app.emit("insert-text", text);
}

// ---- history ----

fn store() -> Result<&'static crate::store::Store, String> {
    STORE.get().ok_or_else(|| "History isn't available (see the log).".to_string())
}

#[tauri::command]
pub fn history_list(query: String, offset: i64) -> Result<Vec<HistoryEntry>, String> {
    store()?.list_history(query.trim(), 100, offset).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn history_delete(app: AppHandle, id: i64) -> Result<(), String> {
    store()?.delete_history(id).map_err(|e| e.to_string())?;
    crate::history_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn history_clear(app: AppHandle) -> Result<(), String> {
    store()?.clear_history().map_err(|e| e.to_string())?;
    crate::history_changed(&app);
    Ok(())
}

/// Put an entry on the clipboard (stays there, unlike dictation's temporary use of it).
#[tauri::command]
pub fn history_copy(id: i64) -> Result<(), String> {
    let text = store()?.history_text(id).map_err(|e| e.to_string())?.ok_or("That entry no longer exists.")?;
    arboard::Clipboard::new().and_then(|mut c| c.set_text(text)).map_err(|e| e.to_string())
}

/// Paste an entry into the app you were using: hide the history window, let focus return, paste.
#[tauri::command]
pub fn history_repaste(app: AppHandle, id: i64) {
    if let Some(w) = app.get_webview_window("history") {
        let _ = w.minimize();
    }
    repaste(&app, id);
}

/// Paste a history entry into whatever window gets focus shortly (tray menu or history window).
pub fn repaste(_app: &AppHandle, id: i64) {
    let Some(text) = STORE.get().and_then(|s| s.history_text(id).ok().flatten()) else { return };
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(350)); // menu/window closes, focus returns
        if winsys::foreground_is_ours() {
            log::info!("re-paste skipped: a Saywell window has focus");
            return;
        }
        if let Err(e) = insert::paste(&text) {
            log::error!("re-paste failed: {e:#}");
        }
    });
}

#[tauri::command]
pub fn open_history(app: AppHandle) {
    open_history_window(&app);
}

// ---- dictionary ----

#[derive(Serialize)]
pub struct DictionaryView {
    terms: Vec<Term>,
    rules: Vec<Rule>,
}

#[tauri::command]
pub fn dict_get() -> Result<DictionaryView, String> {
    let s = store()?;
    Ok(DictionaryView { terms: s.terms().map_err(|e| e.to_string())?, rules: s.rules().map_err(|e| e.to_string())? })
}

#[tauri::command]
pub fn dict_add_term(term: String) -> Result<DictionaryView, String> {
    if term.trim().chars().count() > 60 {
        return Err("Keep each word or name under 60 characters.".into());
    }
    store()?.add_term(&term).map_err(|e| e.to_string())?;
    dict_get()
}

#[tauri::command]
pub fn dict_remove_term(id: i64) -> Result<DictionaryView, String> {
    store()?.remove_term(id).map_err(|e| e.to_string())?;
    dict_get()
}

#[tauri::command]
pub fn dict_set_rule(find: String, replace: String) -> Result<DictionaryView, String> {
    if find.trim().is_empty() {
        return Err("Type what you'll say first.".into());
    }
    store()?.set_rule(&find, &replace).map_err(|e| e.to_string())?;
    dict_get()
}

#[tauri::command]
pub fn dict_remove_rule(id: i64) -> Result<DictionaryView, String> {
    store()?.remove_rule(id).map_err(|e| e.to_string())?;
    dict_get()
}

#[tauri::command]
pub fn engine_status() -> String {
    crate::status_text()
}
