//! Cheap checks before and after speech-to-text: silence detection and Whisper hallucination stripping.

use crate::audio::Clip;

/// Clips shorter than this are treated as accidental taps.
pub const MIN_SECS: f32 = 0.3;
/// RMS below this is treated as silence (full scale = 1.0).
pub const MIN_RMS: f32 = 0.002;

/// Why a clip was skipped before transcription, if it was.
pub fn skip_reason(clip: &Clip) -> Option<&'static str> {
    if clip.duration_secs() < MIN_SECS {
        return Some("too short");
    }
    let rms = (clip.samples.iter().map(|s| s * s).sum::<f32>() / clip.samples.len() as f32).sqrt();
    if rms < MIN_RMS {
        return Some("silence");
    }
    None
}

/// Phrases Whisper tends to invent on silence or noise. Matched against the whole transcript only.
const PHANTOMS: &[&str] = &[
    "thank you.",
    "thank you",
    "thanks for watching!",
    "thanks for watching.",
    "thank you for watching.",
    "you",
    "bye.",
    ".",
];

/// Clean raw Whisper output: drop bracketed markers like `[BLANK_AUDIO]`, collapse whitespace,
/// and return an empty string for known phantom phrases.
pub fn clean_transcript(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut depth = 0i32;
    for ch in raw.chars() {
        match ch {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    let text = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if PHANTOMS.contains(&text.to_lowercase().as_str()) || text.eq_ignore_ascii_case("(silence)") {
        return String::new();
    }
    text
}

/// Hesitation sounds that never carry meaning. Words like "like" or "so" are left to the LLM,
/// because they are often real words.
const FILLERS: &[&str] = &["um", "umm", "uh", "uhh", "uhm", "erm", "hmm", "mm"];

/// Rule-based cleanup used when the LLM is off or its output is rejected: removes hesitation
/// sounds and fixes the capitalisation they leave behind.
pub fn remove_fillers(text: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut cap_next = false;
    for (i, tok) in text.split_whitespace().enumerate() {
        let core: String = tok.chars().filter(|c| c.is_alphabetic()).collect::<String>().to_lowercase();
        if FILLERS.contains(&core.as_str()) {
            let end = tok.chars().last().filter(|c| matches!(c, '.' | '?' | '!'));
            match (end, words.last_mut()) {
                // "it's done, um." -> "it's done."
                (Some(p), Some(prev)) => {
                    let trimmed = prev.trim_end_matches([',', ';']).to_string();
                    *prev = format!("{trimmed}{p}");
                    cap_next = true;
                }
                // "I need, uh, milk" -> "I need milk"
                (None, Some(prev)) if tok.ends_with(',') && prev.ends_with(',') => {
                    prev.pop();
                }
                // Filler started the text or a sentence: capitalise what follows.
                _ => cap_next = cap_next || i == 0 || words.last().is_some_and(|w| w.ends_with(['.', '?', '!'])),
            }
            continue;
        }
        let mut w = tok.to_string();
        if cap_next {
            let mut cs = w.chars();
            if let Some(f) = cs.next() {
                w = f.to_uppercase().chain(cs).collect();
            }
            cap_next = false;
        }
        words.push(w);
    }
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_short_and_silent_clips() {
        assert_eq!(skip_reason(&Clip { samples: vec![0.5; 1000] }), Some("too short"));
        assert_eq!(skip_reason(&Clip { samples: vec![0.0001; 16_000] }), Some("silence"));
        let tone: Vec<f32> = (0..16_000).map(|i| (i as f32 / 10.0).sin() * 0.1).collect();
        assert_eq!(skip_reason(&Clip { samples: tone }), None);
    }

    #[test]
    fn strips_markers_and_whitespace() {
        assert_eq!(clean_transcript(" [BLANK_AUDIO]  Hello   world. \n"), "Hello world.");
        assert_eq!(clean_transcript("[MUSIC] [BLANK_AUDIO]"), "");
    }

    #[test]
    fn drops_phantom_phrases_only_when_alone() {
        assert_eq!(clean_transcript(" Thank you."), "");
        assert_eq!(clean_transcript("you"), "");
        assert_eq!(clean_transcript("Thank you for the update."), "Thank you for the update.");
    }

    #[test]
    fn removes_hesitation_sounds() {
        assert_eq!(remove_fillers("Um, so the meeting is at four."), "So the meeting is at four.");
        assert_eq!(remove_fillers("I need, uh, milk and eggs."), "I need milk and eggs.");
        assert_eq!(remove_fillers("It works. Uh, mostly."), "It works. Mostly.");
        assert_eq!(remove_fillers("That's it, um."), "That's it.");
        assert_eq!(remove_fillers("Summer is umbrella season."), "Summer is umbrella season.");
        assert_eq!(remove_fillers("Um."), "");
    }
}
