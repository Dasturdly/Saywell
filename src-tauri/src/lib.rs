//! Saywell: hold a hotkey, speak, release, and the text is pasted into the focused app.
//!
//! M1 pipeline: hotkey hook → mic capture → whisper-server (Vulkan) → paste.

pub mod audio;
pub mod hotkey;
pub mod insert;
pub mod postprocess;
pub mod settings;
pub mod sidecar;
pub mod stt;

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use anyhow::Result;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, RunEvent};

use audio::{Clip, Recorder};
use hotkey::HotkeyEvent;
use settings::Settings;
use stt::Stt;

static STT: OnceLock<Arc<Stt>> = OnceLock::new();

/// Command-line options (developer and test modes).
#[derive(Debug, Default)]
struct Cli {
    /// Transcribe these WAV files, print results and exit (no UI).
    transcribe: Vec<PathBuf>,
    /// Use this WAV file instead of the microphone for every dictation.
    inject_wav: Option<PathBuf>,
}

fn parse_cli() -> Cli {
    let mut cli = Cli::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--transcribe" => cli.transcribe.extend(args.by_ref().map(PathBuf::from)),
            "--inject-wav" => cli.inject_wav = args.next().map(PathBuf::from),
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
    log::info!("Saywell {} starting; models in {}", env!("CARGO_PKG_VERSION"), settings.models_dir.display());

    if !cli.transcribe.is_empty() {
        std::process::exit(match run_transcribe(&settings, &cli.transcribe) {
            Ok(()) => 0,
            Err(e) => {
                log::error!("{e:#}");
                1
            }
        });
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

fn run_app(settings: Settings, cli: Cli) {
    let app = tauri::Builder::default()
        .setup(move |app| {
            build_tray(app.handle())?;
            start_engines(app.handle().clone(), settings, cli.inject_wav);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build Tauri app");

    app.run(|_, event| {
        if let RunEvent::Exit = event {
            if let Some(stt) = STT.get() {
                stt.stop();
            }
        }
    });
}

fn build_tray(app: &AppHandle) -> Result<()> {
    let quit = MenuItem::with_id(app, "quit", "Quit Saywell", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&quit])?;
    TrayIconBuilder::with_id("main")
        .icon(Image::from_bytes(include_bytes!("../icons/icon.png"))?)
        .tooltip("Saywell: loading speech model…")
        .menu(&menu)
        .on_menu_event(|app, event| {
            if event.id().as_ref() == "quit" {
                app.exit(0);
            }
        })
        .build(app)?;
    Ok(())
}

fn set_status(app: &AppHandle, text: &str) {
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(format!("Saywell: {text}")));
    }
}

fn start_engines(app: AppHandle, settings: Settings, inject_wav: Option<PathBuf>) {
    let model = settings.whisper_model_path();
    let app2 = app.clone();
    std::thread::Builder::new()
        .name("stt-start".into())
        .spawn(move || match Stt::start(&model, &log_dir()) {
            Ok(stt) => {
                let _ = STT.set(Arc::new(stt));
                set_status(&app2, "ready. Hold Right Ctrl to dictate");
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
    let injected = inject_wav.map(|p| {
        let clip = audio::read_wav(&p).unwrap_or_else(|e| panic!("--inject-wav {}: {e:#}", p.display()));
        log::warn!("TEST MODE: microphone replaced by {} ({:.1}s)", p.display(), clip.duration_secs());
        clip
    });

    std::thread::Builder::new()
        .name("controller".into())
        .spawn(move || controller(app, rx, recorder, injected))
        .expect("spawn controller");
}

/// Reacts to hotkey events. Transcription runs on a worker thread so the hotkey stays responsive.
fn controller(app: AppHandle, rx: mpsc::Receiver<HotkeyEvent>, recorder: Recorder, injected: Option<Clip>) {
    let busy = Arc::new(AtomicBool::new(false));
    let mut recording_since: Option<Instant> = None;
    for ev in rx {
        match ev {
            HotkeyEvent::Pressed => {
                if busy.load(Ordering::SeqCst) {
                    log::info!("still processing the previous dictation; ignoring press");
                    continue;
                }
                recorder.start();
                recording_since = Some(Instant::now());
                set_status(&app, "listening…");
            }
            HotkeyEvent::Cancel => {
                if recording_since.take().is_some() {
                    let _ = recorder.stop();
                    set_status(&app, "cancelled");
                    log::info!("dictation cancelled");
                }
            }
            HotkeyEvent::Released => {
                let Some(since) = recording_since.take() else { continue };
                let released = Instant::now();
                let mic = recorder.stop();
                let clip = match (&injected, mic) {
                    (Some(c), _) => c.clone(),
                    (None, Ok(c)) => c,
                    (None, Err(e)) => {
                        log::error!("recording failed: {e:#}");
                        set_status(&app, "microphone error (see log)");
                        continue;
                    }
                };
                log::info!("held {:.2}s, captured {:.2}s of audio", since.elapsed().as_secs_f32(), clip.duration_secs());
                busy.store(true, Ordering::SeqCst);
                let (app, busy) = (app.clone(), busy.clone());
                std::thread::spawn(move || {
                    process(&app, clip, released);
                    busy.store(false, Ordering::SeqCst);
                });
            }
        }
    }
}

fn process(app: &AppHandle, clip: Clip, released: Instant) {
    if let Some(reason) = postprocess::skip_reason(&clip) {
        log::info!("skipped clip: {reason}");
        set_status(app, "ready");
        return;
    }
    let Some(stt) = STT.get() else {
        log::warn!("speech model still loading; dictation dropped");
        set_status(app, "still loading the speech model…");
        return;
    };
    set_status(app, "transcribing…");
    let t_stt = Instant::now();
    let raw = match stt.transcribe(&clip) {
        Ok(t) => t,
        Err(e) => {
            log::error!("transcription failed: {e:#}");
            set_status(app, "transcription error (see log)");
            return;
        }
    };
    let stt_ms = t_stt.elapsed().as_millis();
    let text = postprocess::clean_transcript(&raw);
    if text.is_empty() {
        log::info!("nothing recognised (stt {stt_ms} ms)");
        set_status(app, "ready");
        return;
    }
    let t_ins = Instant::now();
    if let Err(e) = insert::paste(&text) {
        log::error!("insert failed: {e:#}");
        set_status(app, "could not paste (see log)");
        return;
    }
    log::info!(
        "dictation: audio={:.2}s stt={}ms insert={}ms release_to_text={}ms chars={}",
        clip.duration_secs(),
        stt_ms,
        t_ins.elapsed().as_millis(),
        released.elapsed().as_millis(),
        text.chars().count()
    );
    set_status(app, "ready. Hold Right Ctrl to dictate");
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
