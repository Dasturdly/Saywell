//! The dictation state machine: hotkey events in, overlay updates and pasted text out.
//!
//! Hold the hotkey to talk; release to transcribe, clean up and paste. A quick tap followed by a
//! second press within 400 ms switches to hands-free: recording continues until the next press.
//! Esc cancels. While recording, a preview of the words so far is shown about once a second.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::AppHandle;

use crate::audio::{Clip, Recorder};
use crate::hotkey::{self, HotkeyEvent};
use crate::{cleanup, insert, postprocess, settings_snapshot, ui, LLM, STT};

/// A press shorter than this may be the first half of a double-tap.
const TAP_MAX: Duration = Duration::from_millis(300);
/// The second tap must start within this time after the first one ends.
const DOUBLE_TAP_GAP: Duration = Duration::from_millis(400);
const PREVIEW_EVERY: Duration = Duration::from_millis(1000);
/// The preview only re-transcribes the most recent audio, so it stays fast on long dictations.
const PREVIEW_WINDOW_SECS: f32 = 20.0;
const TICK: Duration = Duration::from_millis(50);

/// Bumped whenever a recording ends, so late preview results for it are dropped.
static SESSION: AtomicU64 = AtomicU64::new(0);

struct Rec {
    since: Instant,
    hands_free: bool,
    /// Set after a short tap: if no second press arrives by then, the tap is discarded.
    tap_deadline: Option<Instant>,
    last_preview: Instant,
}

pub struct Controller {
    app: AppHandle,
    recorder: Recorder,
    /// Test mode: this clip replaces the microphone.
    injected: Option<Clip>,
    rec: Option<Rec>,
    ignore_release: bool,
    busy: Arc<AtomicBool>,
    preview_busy: Arc<AtomicBool>,
}

impl Controller {
    pub fn new(app: AppHandle, recorder: Recorder, injected: Option<Clip>) -> Self {
        Self {
            app,
            recorder,
            injected,
            rec: None,
            ignore_release: false,
            busy: Arc::new(AtomicBool::new(false)),
            preview_busy: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn run(mut self, rx: Receiver<HotkeyEvent>) {
        loop {
            match rx.recv_timeout(TICK) {
                Ok(ev) => self.on_event(ev),
                Err(RecvTimeoutError::Timeout) => self.tick(),
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    fn on_event(&mut self, ev: HotkeyEvent) {
        log::debug!("controller got {ev:?}");
        match ev {
            HotkeyEvent::Pressed => self.on_press(),
            HotkeyEvent::Released => self.on_release(),
            HotkeyEvent::Cancel => self.cancel(),
        }
    }

    fn on_press(&mut self) {
        if let Some(rec) = &mut self.rec {
            if rec.hands_free {
                self.ignore_release = true;
                self.finish();
            } else if rec.tap_deadline.take().is_some() {
                rec.hands_free = true;
                self.ignore_release = true;
                hotkey::set_esc_armed(true);
                log::info!("hands-free recording");
                ui::overlay(&self.app, "handsfree", "");
            }
            return;
        }
        if self.busy.load(Ordering::SeqCst) {
            log::info!("still processing the previous dictation; ignoring press");
            return;
        }
        if self.injected.is_none() {
            self.recorder.start(settings_snapshot().mic_device); // test mode never opens the mic
        }
        let now = Instant::now();
        self.rec = Some(Rec { since: now, hands_free: false, tap_deadline: None, last_preview: now });
        ui::overlay(&self.app, "listening", "");
    }

    fn on_release(&mut self) {
        if std::mem::take(&mut self.ignore_release) {
            return;
        }
        let Some(rec) = &mut self.rec else { return };
        if rec.hands_free {
            return;
        }
        if settings_snapshot().double_tap_hands_free && rec.since.elapsed() < TAP_MAX && rec.tap_deadline.is_none() {
            rec.tap_deadline = Some(Instant::now() + DOUBLE_TAP_GAP); // keep recording; maybe a double-tap
            return;
        }
        self.finish();
    }

    fn cancel(&mut self) {
        if self.rec.take().is_some() {
            SESSION.fetch_add(1, Ordering::SeqCst);
            hotkey::set_esc_armed(false);
            self.ignore_release = false;
            if self.injected.is_none() {
                let _ = self.recorder.stop();
            }
            log::info!("dictation cancelled");
            ui::overlay(&self.app, "hidden", "");
        }
    }

    fn tick(&mut self) {
        let Some(rec) = &self.rec else { return };
        if rec.tap_deadline.is_some_and(|d| Instant::now() >= d) {
            log::info!("single short tap; discarded");
            self.cancel();
            return;
        }
        let elapsed = rec.since.elapsed();
        let level = match &self.injected {
            Some(_) => 0.05 + 0.04 * (elapsed.as_secs_f32() * 9.0).sin().abs(),
            None => self.recorder.level(),
        };
        ui::overlay_level(&self.app, level);

        let preview_due = rec.last_preview.elapsed() >= PREVIEW_EVERY && elapsed >= PREVIEW_EVERY;
        if preview_due && rec.tap_deadline.is_none() && !self.preview_busy.load(Ordering::SeqCst) {
            if let Some(rec) = &mut self.rec {
                rec.last_preview = Instant::now();
            }
            self.start_preview(elapsed);
        }
    }

    /// Transcribe the audio so far on a worker thread and show it in the overlay.
    fn start_preview(&self, elapsed: Duration) {
        if !settings_snapshot().preview_caption {
            return;
        }
        let Some(stt) = STT.get().cloned() else { return };
        let clip = match &self.injected {
            Some(c) => {
                let n = ((elapsed.as_secs_f32() * crate::audio::TARGET_RATE as f32) as usize).min(c.samples.len());
                let start = n.saturating_sub((PREVIEW_WINDOW_SECS * crate::audio::TARGET_RATE as f32) as usize);
                Some(Clip { samples: c.samples[start..n].to_vec() })
            }
            None => self.recorder.peek(PREVIEW_WINDOW_SECS),
        };
        let Some(clip) = clip.filter(|c| postprocess::skip_reason(c).is_none()) else { return };
        let session = SESSION.load(Ordering::SeqCst);
        let (app, busy) = (self.app.clone(), self.preview_busy.clone());
        busy.store(true, Ordering::SeqCst);
        std::thread::spawn(move || {
            let t0 = Instant::now();
            if let Ok(raw) = stt.transcribe(&clip) {
                let text = postprocess::clean_transcript(&raw);
                log::info!(
                    "preview: {:.1}s of audio, {} chars in {} ms",
                    clip.duration_secs(),
                    text.chars().count(),
                    t0.elapsed().as_millis()
                );
                if SESSION.load(Ordering::SeqCst) == session && !text.is_empty() {
                    ui::overlay_preview(&app, &text);
                }
            }
            busy.store(false, Ordering::SeqCst);
        });
    }

    /// Stop recording and process the clip on a worker thread.
    fn finish(&mut self) {
        let Some(rec) = self.rec.take() else { return };
        SESSION.fetch_add(1, Ordering::SeqCst);
        hotkey::set_esc_armed(false);
        let released = Instant::now();
        let clip = match self.injected.as_ref().map(|c| Ok(c.clone())).unwrap_or_else(|| self.recorder.stop()) {
            Ok(c) => c,
            Err(e) => {
                log::error!("recording failed: {e:#}");
                ui::overlay_error(&self.app, "Microphone problem. See the log.");
                return;
            }
        };
        let (peak, rms) = clip.levels();
        log::info!(
            "{} {:.2}s, captured {:.2}s of audio (peak {peak:.3}, rms {rms:.4})",
            if rec.hands_free { "hands-free" } else { "held" },
            rec.since.elapsed().as_secs_f32(),
            clip.duration_secs()
        );
        self.busy.store(true, Ordering::SeqCst);
        ui::overlay(&self.app, "thinking", "");
        let (app, busy) = (self.app.clone(), self.busy.clone());
        std::thread::spawn(move || {
            match process(&app, &clip, released) {
                Ok(()) => ui::overlay(&app, "hidden", ""),
                Err(msg) => ui::overlay_error(&app, &msg),
            }
            busy.store(false, Ordering::SeqCst);
        });
    }
}

/// Transcribe, clean up and paste. `Err` carries a short message for the overlay.
fn process(app: &AppHandle, clip: &Clip, released: Instant) -> Result<(), String> {
    if let Some(reason) = postprocess::skip_reason(clip) {
        log::info!("skipped clip: {reason}");
        return if reason == "silence" { Err("Didn't hear anything. Check your microphone.".into()) } else { Ok(()) };
    }
    let Some(stt) = STT.get() else {
        log::warn!("speech model still loading; dictation dropped");
        return Err("Still loading the speech model…".into());
    };
    let t_stt = Instant::now();
    let raw = stt.transcribe(clip).map_err(|e| {
        log::error!("transcription failed: {e:#}");
        "Transcription failed. See the log.".to_string()
    })?;
    let stt_ms = t_stt.elapsed().as_millis();
    let transcript = postprocess::clean_transcript(&raw);
    if transcript.is_empty() {
        crate::debug_capture(clip, &raw, "");
        log::info!("nothing recognised (stt {stt_ms} ms)");
        return Err("Didn't catch any words.".into());
    }
    let enabled = settings_snapshot().cleanup_enabled;
    let llm = LLM.lock().unwrap().clone();
    let t_llm = Instant::now();
    let cleaned = cleanup::clean(llm.as_ref(), &transcript, enabled);
    let llm_ms = t_llm.elapsed().as_millis();
    let source = match &cleaned.source {
        cleanup::Source::Llm => "llm",
        cleanup::Source::Off => "off",
        cleanup::Source::Short => "short",
        cleanup::Source::Fallback(why) => {
            log::warn!("cleanup fell back to rules: {why}");
            "fallback"
        }
    };
    let text = cleaned.text;
    crate::debug_capture(clip, &raw, &text);
    if text.is_empty() {
        return Ok(());
    }
    let t_ins = Instant::now();
    if crate::winsys::foreground_is_ours() {
        ui::insert_into_own_window(app, &text);
    } else {
        insert::paste(&text).map_err(|e| {
            log::error!("insert failed: {e:#}");
            "Couldn't paste here. The text is not on your clipboard.".to_string()
        })?;
    }
    log::info!(
        "dictation: audio={:.2}s stt={}ms cleanup={}ms ({source}) insert={}ms release_to_text={}ms chars={}",
        clip.duration_secs(),
        stt_ms,
        llm_ms,
        t_ins.elapsed().as_millis(),
        released.elapsed().as_millis(),
        text.chars().count()
    );
    Ok(())
}
