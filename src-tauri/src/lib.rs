//! Saywell: hold a hotkey, speak, release, and the text is pasted into the focused app.
//!
//! Pipeline: hotkey hook → mic capture → whisper-server (Vulkan) → LLM cleanup (llama-server,
//! optional) → paste. A small overlay shows what is happening; nothing leaves the PC.

pub mod audio;
pub mod cleanup;
pub mod controller;
pub mod hotkey;
pub mod insert;
pub mod llm;
pub mod models;
pub mod postprocess;
pub mod settings;
pub mod sidecar;
pub mod stt;
pub mod ui;
pub mod winsys;

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use anyhow::Result;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, RunEvent, Wry};

use audio::{Clip, Recorder};
use llm::Llm;
use settings::Settings;
use stt::Stt;

static STT: OnceLock<Arc<Stt>> = OnceLock::new();
/// Running cleanup LLM; `None` while loading, after a failed start, or when cleanup is off.
static LLM: Mutex<Option<Arc<Llm>>> = Mutex::new(None);
static LLM_STARTING: AtomicBool = AtomicBool::new(false);
static SETTINGS: OnceLock<Mutex<Settings>> = OnceLock::new();
static ENGINES_STARTED: AtomicBool = AtomicBool::new(false);
/// `--inject-wav` test clip, replacing the microphone.
static INJECTED: OnceLock<Option<Clip>> = OnceLock::new();
static STATUS: Mutex<String> = Mutex::new(String::new());

/// Tray checkbox for cleanup, kept so the settings window can update it.
struct TrayCleanupItem(CheckMenuItem<Wry>);

fn settings_snapshot() -> Settings {
    SETTINGS.get().map(|s| s.lock().unwrap().clone()).unwrap_or_default()
}

/// Command-line options (developer and test modes).
#[derive(Debug, Default)]
struct Cli {
    /// Transcribe these WAV files, print results and exit (no UI).
    transcribe: Vec<PathBuf>,
    /// Use this WAV file instead of the microphone for every dictation.
    inject_wav: Option<PathBuf>,
    /// Run LLM cleanup on each line of this file (`raw|expected` or just raw), print and exit.
    clean_file: Option<PathBuf>,
    /// Show first-run setup even if the models are present.
    setup: bool,
    /// Open the settings window at start.
    settings: bool,
}

fn parse_cli() -> Cli {
    let mut cli = Cli::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--transcribe" => cli.transcribe.extend(args.by_ref().map(PathBuf::from)),
            "--inject-wav" => cli.inject_wav = args.next().map(PathBuf::from),
            "--clean-file" => cli.clean_file = args.next().map(PathBuf::from),
            "--setup" => cli.setup = true,
            "--settings" => cli.settings = true,
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }
    cli
}

pub fn main_entry() {
    init_logging();
    let cli = parse_cli();
    let settings = match Settings::load_or_create(&settings::settings_path()) {
        Ok(s) => s,
        Err(e) => {
            log::error!("bad settings file, using defaults: {e:#}");
            Settings::default()
        }
    };
    log::info!(
        "Saywell {} starting; settings {}; models in {}",
        env!("CARGO_PKG_VERSION"),
        settings::settings_path().display(),
        settings.models_dir.display()
    );

    let headless = if let Some(file) = &cli.clean_file {
        Some(run_clean_file(&settings, file))
    } else if !cli.transcribe.is_empty() {
        Some(run_transcribe(&settings, &cli.transcribe))
    } else {
        None
    };
    if let Some(result) = headless {
        std::process::exit(match result {
            Ok(()) => 0,
            Err(e) => {
                log::error!("{e:#}");
                1
            }
        });
    }
    if !winsys::acquire_single_instance() {
        log::warn!("another Saywell is already running; exiting");
        winsys::message_box("Saywell", "Saywell is already running. Look for its icon in the system tray.");
        return;
    }
    run_app(settings, cli);
}

/// Headless mode: `saywell --transcribe a.wav b.wav` prints `<seconds>\t<file>\t<text>` per file.
fn run_transcribe(settings: &Settings, files: &[PathBuf]) -> Result<()> {
    let stt = Stt::start(&settings.whisper_model_path(), &log_dir())?;
    for f in files {
        let clip = audio::read_wav(f)?;
        let t0 = Instant::now();
        let text = postprocess::clean_transcript(&stt.transcribe(&clip)?);
        println!("{:.3}\t{}\t{}", t0.elapsed().as_secs_f32(), f.display(), text);
    }
    stt.stop();
    Ok(())
}

/// Headless mode: `saywell --clean-file cases.tsv` prints `<seconds>\t<source>\t<raw>\t<cleaned>`.
fn run_clean_file(settings: &Settings, file: &std::path::Path) -> Result<()> {
    let llm = Llm::start(&settings.llm_model_path(), &log_dir())?;
    for line in std::fs::read_to_string(file)?.lines().filter(|l| !l.trim().is_empty()) {
        let raw = line.split('|').next().unwrap_or_default().trim_start_matches('\u{feff}');
        let t0 = Instant::now();
        let c = cleanup::clean(Some(&llm), raw, true);
        println!("{:.3}\t{:?}\t{raw}\t{}", t0.elapsed().as_secs_f32(), c.source, c.text);
    }
    llm.stop();
    Ok(())
}

fn run_app(settings: Settings, cli: Cli) {
    let _ = SETTINGS.set(Mutex::new(settings.clone()));
    let injected = cli.inject_wav.as_ref().map(|p| {
        let clip = audio::read_wav(p).unwrap_or_else(|e| panic!("--inject-wav {}: {e:#}", p.display()));
        log::warn!("TEST MODE: microphone replaced by {} ({:.1}s)", p.display(), clip.duration_secs());
        clip
    });
    let _ = INJECTED.set(injected);

    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            ui::get_settings,
            ui::update_settings,
            ui::capture_hotkey,
            ui::setup_status,
            ui::start_download,
            ui::cancel_download,
            ui::open_models_dir,
            ui::finish_setup,
            ui::close_setup,
            ui::engine_status,
            ui::page_key,
        ])
        .setup(move |app| {
            let handle = app.handle();
            build_tray(handle)?;
            ui::create_overlay(handle)?;
            let models_ready = settings.whisper_model_path().exists();
            if cli.setup || !models_ready {
                log::info!("showing first-run setup (speech model present: {models_ready})");
                set_status(handle, "setup needed");
                ui::open_onboarding(handle);
            } else {
                start_engines(handle);
            }
            if cli.settings {
                ui::open_settings(handle);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build Tauri app");

    app.run(|_, event| match event {
        // Closing the settings or setup window must not quit the tray app.
        RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
        RunEvent::Exit => {
            if let Some(stt) = STT.get() {
                stt.stop();
            }
            if let Some(llm) = LLM.lock().unwrap().take() {
                llm.stop();
            }
        }
        _ => {}
    });
}

fn build_tray(app: &AppHandle) -> Result<()> {
    let enabled = settings_snapshot().cleanup_enabled;
    let settings_item = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let cleanup = CheckMenuItem::with_id(app, "cleanup", "Clean up text with local AI", true, enabled, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Saywell", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&settings_item, &cleanup, &sep, &quit])?;
    app.manage(TrayCleanupItem(cleanup.clone()));
    TrayIconBuilder::with_id("main")
        .icon(Image::from_bytes(include_bytes!("../icons/icon.png"))?)
        .tooltip("Saywell: starting…")
        .menu(&menu)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "quit" => app.exit(0),
            "settings" => ui::open_settings(app),
            "cleanup" => set_cleanup(app, cleanup.is_checked().unwrap_or(false)),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

fn set_status(app: &AppHandle, text: &str) {
    *STATUS.lock().unwrap() = text.to_string();
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(format!("Saywell: {text}")));
    }
    let _ = app.emit("status", text);
}

fn status_text() -> String {
    STATUS.lock().unwrap().clone()
}

/// Re-render the "ready" status (e.g. after the hotkey changed).
fn refresh_status(app: &AppHandle) {
    if STT.get().is_some() {
        set_status(app, &format!("ready. Hold {} to dictate", settings_snapshot().hotkey_label));
    }
}

/// Turn LLM cleanup on or off, save the choice, sync the tray checkbox, and start or stop
/// llama-server to match (stopping it frees about 3.5 GB of VRAM).
fn set_cleanup(app: &AppHandle, on: bool) {
    if let Some(s) = SETTINGS.get() {
        let mut s = s.lock().unwrap();
        s.cleanup_enabled = on;
        if let Err(e) = s.save(&settings::settings_path()) {
            log::error!("could not save settings: {e:#}");
        }
    }
    if let Some(item) = app.try_state::<TrayCleanupItem>() {
        let _ = item.0.set_checked(on);
    }
    log::info!("cleanup {}", if on { "on" } else { "off" });
    if !ENGINES_STARTED.load(Ordering::SeqCst) {
        return; // start_engines will start it
    }
    if on {
        start_llm(settings_snapshot());
    } else if let Some(llm) = LLM.lock().unwrap().take() {
        llm.stop();
    }
}

fn start_llm(settings: Settings) {
    if LLM.lock().unwrap().is_some() || LLM_STARTING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("llm-start".into())
        .spawn(move || {
            match Llm::start(&settings.llm_model_path(), &log_dir()) {
                Ok(llm) => {
                    let mut slot = LLM.lock().unwrap();
                    if settings_snapshot().cleanup_enabled && slot.is_none() {
                        *slot = Some(llm);
                        log::info!("cleanup model ready");
                    } else {
                        llm.stop(); // turned off while loading
                    }
                }
                Err(e) => log::error!("cleanup model failed to start; using rule-based cleanup: {e:#}"),
            }
            LLM_STARTING.store(false, Ordering::SeqCst);
        })
        .expect("spawn llm-start");
}

/// Start speech recognition, cleanup, the hotkey hook and the controller. Safe to call twice.
fn start_engines(app: &AppHandle) {
    if ENGINES_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let settings = settings_snapshot();
    if settings.cleanup_enabled {
        start_llm(settings.clone());
    }
    set_status(app, "loading speech model…");
    let model = settings.whisper_model_path();
    let app2 = app.clone();
    std::thread::Builder::new()
        .name("stt-start".into())
        .spawn(move || match Stt::start(&model, &log_dir()) {
            Ok(stt) => {
                let _ = STT.set(Arc::new(stt));
                refresh_status(&app2);
                log::info!("ready");
            }
            Err(e) => {
                set_status(&app2, "speech model failed to load (see log)");
                log::error!("speech engine failed to start: {e:#}");
            }
        })
        .expect("spawn stt-start");

    let (tx, rx) = mpsc::channel();
    hotkey::spawn(settings.hotkey_vk, tx);
    let recorder = Recorder::spawn(settings.max_record_secs);
    let injected = INJECTED.get().cloned().flatten();
    let ctl = controller::Controller::new(app.clone(), recorder, injected);
    std::thread::Builder::new()
        .name("controller".into())
        .spawn(move || ctl.run(rx))
        .expect("spawn controller");
}

/// With `SAYWELL_DEBUG_AUDIO=1`, keep each dictation (16 kHz WAV, raw Whisper text, final text) for
/// replaying with `--transcribe`. Off by default: normally no audio or transcript is stored.
/// Debug builds write to the repo's git-ignored `tests/fixtures/audio/user/`, release builds to
/// `%APPDATA%\Saywell\debug-audio`.
fn debug_capture(clip: &Clip, raw: &str, text: &str) {
    if std::env::var_os("SAYWELL_DEBUG_AUDIO").is_none_or(|v| v.is_empty() || v == "0") {
        return;
    }
    let dir = if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/audio/user")
    } else {
        settings::data_dir().join("debug-audio")
    };
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let base = dir.join(format!("mic-{stamp}"));
    let res = (|| -> Result<()> {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(base.with_extension("wav"), audio::encode_wav(clip)?)?;
        std::fs::write(base.with_extension("raw.txt"), raw)?;
        std::fs::write(base.with_extension("txt"), text)?;
        Ok(())
    })();
    match res {
        Ok(()) => log::info!("debug capture saved to {}.wav", base.display()),
        Err(e) => log::warn!("debug capture failed: {e:#}"),
    }
}

pub fn log_dir() -> PathBuf {
    settings::data_dir().join("logs")
}

/// Log to stderr and `%APPDATA%\Saywell\logs\saywell.log`. Transcript text is never logged.
fn init_logging() {
    struct Tee(Option<std::fs::File>);
    impl Write for Tee {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let _ = std::io::stderr().write_all(buf);
            if let Some(f) = &mut self.0 {
                let _ = f.write_all(buf);
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            if let Some(f) = &mut self.0 {
                f.flush()?;
            }
            Ok(())
        }
    }
    let _ = std::fs::create_dir_all(log_dir());
    let file = std::fs::OpenOptions::new().create(true).append(true).open(log_dir().join("saywell.log")).ok();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Pipe(Box::new(Tee(file))))
        .init();
}
