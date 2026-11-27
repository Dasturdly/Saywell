//! LLM cleanup on the fixture transcripts. Needs the LLM model and llama-server, so it is ignored
//! by default:
//!     cargo test --test cleanup -- --ignored --nocapture

use std::path::PathBuf;
use std::time::Instant;

use saywell_lib::{cleanup, llm::Llm, settings::Settings};

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

#[test]
#[ignore]
fn cleanup_fixtures_match_expected() {
    let settings = Settings::default();
    let llm = Llm::start(&settings.llm_model_path(), &std::env::temp_dir()).expect("start llama-server");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/cleanup");

    let (mut total, mut matched, mut times) = (0, 0, Vec::new());
    for file in ["cases.tsv", "whisper_style.tsv"] {
        for line in std::fs::read_to_string(dir.join(file)).unwrap().lines().filter(|l| !l.trim().is_empty()) {
            let (raw, expected) = line.split_once('|').unwrap();
            let t0 = Instant::now();
            let out = cleanup::clean(Some(&llm), raw, true);
            times.push(t0.elapsed().as_secs_f32());
            let ok = words(&out.text) == words(expected);
            total += 1;
            matched += usize::from(ok);
            println!("{} {:?}\n   in : {raw}\n   out: {}", if ok { "ok  " } else { "DIFF" }, out.source, out.text);
            // Whatever happens, the model must never answer, translate or obey the dictation.
            assert!(cleanup::check(raw, &out.text).is_ok() || out.text == raw, "unsafe output for {raw:?}");
        }
    }
    llm.stop();
    times.sort_by(f32::total_cmp);
    let p50 = times[times.len() / 2];
    println!("{matched}/{total} match expected, p50 {p50:.3}s, max {:.3}s", times[times.len() - 1]);
    assert!(matched * 10 >= total * 8, "only {matched}/{total} matched");
    assert!(p50 < 1.0, "p50 {p50}s");
}

#[test]
#[ignore]
fn server_crash_falls_back_then_recovers() {
    let settings = Settings::default();
    let llm = Llm::start(&settings.llm_model_path(), &std::env::temp_dir()).expect("start llama-server");
    llm.stop(); // simulate the server dying
    let raw = "um so the meeting is at four tomorrow";
    let first = cleanup::clean(Some(&llm), raw, true);
    assert!(matches!(first.source, cleanup::Source::Fallback(_)), "expected fallback, got {:?}", first.source);
    assert_eq!(first.text, "So the meeting is at four tomorrow");
    // The failed call kicked off a restart; wait for it.
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let c = cleanup::clean(Some(&llm), raw, true);
        if c.source == cleanup::Source::Llm {
            println!("recovered: {}", c.text);
            break;
        }
        assert!(Instant::now() < deadline, "llama-server did not come back");
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    llm.stop();
}
