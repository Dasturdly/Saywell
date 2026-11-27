//! Full speech pipeline on the fixture clips: WAV → 16 kHz → whisper-server (Vulkan) → cleanup.
//! Needs the whisper model and the built sidecar, so it is ignored by default:
//!     cargo test --test pipeline -- --ignored --nocapture

use std::path::PathBuf;
use std::time::Instant;

use saywell_lib::{audio, postprocess, settings::Settings, stt::Stt};

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .replace('\'', "")
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

fn wer(reference: &str, hypothesis: &str) -> f32 {
    let (r, h) = (words(reference), words(hypothesis));
    let mut d: Vec<usize> = (0..=h.len()).collect();
    for i in 1..=r.len() {
        let mut prev = d[0];
        d[0] = i;
        for j in 1..=h.len() {
            let cur = d[j];
            d[j] = (d[j] + 1).min(d[j - 1] + 1).min(prev + usize::from(r[i - 1] != h[j - 1]));
            prev = cur;
        }
    }
    d[h.len()] as f32 / r.len().max(1) as f32
}

#[test]
#[ignore]
fn fixtures_transcribe_accurately_and_fast() {
    let settings = Settings::load_or_create(&saywell_lib::settings::settings_path()).unwrap();
    let stt = Stt::start(&settings.whisper_model_path(), &std::env::temp_dir()).expect("start whisper-server");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/audio");

    let mut total_wer = 0.0;
    let mut latencies = Vec::new();
    for i in 1..=20 {
        let id = format!("tts{i:02}");
        let clip = audio::read_wav(&dir.join(format!("{id}.wav"))).unwrap();
        assert!(postprocess::skip_reason(&clip).is_none(), "{id} wrongly treated as silence");
        let reference = std::fs::read_to_string(dir.join(format!("{id}.txt"))).unwrap();
        let t0 = Instant::now();
        let text = postprocess::clean_transcript(&stt.transcribe(&clip).unwrap());
        latencies.push(t0.elapsed().as_secs_f32());
        let w = wer(reference.trim_start_matches('\u{feff}'), &text);
        total_wer += w;
        println!("{id} {:.3}s wer={w:.2} {text}", latencies.last().unwrap());
    }
    stt.stop();
    latencies.sort_by(f32::total_cmp);
    let mean_wer = total_wer / 20.0;
    let p50 = latencies[10];
    println!("mean WER {:.1}%  p50 {p50:.3}s  max {:.3}s", mean_wer * 100.0, latencies[19]);
    // Raw WER counts digits ("40%") against words ("forty percent"), so allow some slack.
    assert!(mean_wer < 0.08, "mean WER {mean_wer}");
    assert!(p50 < 1.0, "p50 latency {p50}s");
}

#[test]
#[ignore]
fn silence_is_skipped() {
    let clip = audio::Clip { samples: vec![0.0; 32_000] };
    assert_eq!(postprocess::skip_reason(&clip), Some("silence"));
}

/// The personal dictionary should steer Whisper towards the user's spellings.
#[test]
#[ignore]
fn dictionary_terms_fix_spellings() {
    let settings = Settings::load_or_create(&saywell_lib::settings::settings_path()).unwrap();
    let stt = Stt::start(&settings.whisper_model_path(), &std::env::temp_dir()).expect("start whisper-server");
    let wav = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/audio/dict01.wav");
    let clip = audio::read_wav(&wav).unwrap();
    let terms: Vec<String> = ["Quorvex", "Anneliese", "Kowalczyk", "Kubernetes"].map(String::from).to_vec();
    let prompt = saywell_lib::dictionary::whisper_prompt(&terms);
    let plain = postprocess::clean_transcript(&stt.transcribe(&clip).unwrap());
    let hinted = postprocess::clean_transcript(&stt.transcribe_with_prompt(&clip, &prompt).unwrap());
    stt.stop();
    println!("without dictionary: {plain}\nwith dictionary:    {hinted}");
    let hits = |s: &str| terms.iter().filter(|t| s.contains(t.as_str())).count();
    println!("terms spelled right: {} -> {}", hits(&plain), hits(&hinted));
    assert!(hits(&hinted) >= hits(&plain));
    assert!(hits(&hinted) >= 3, "dictionary should get most terms right: {hinted}");
}
