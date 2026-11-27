//! Personal dictionary: spelling hints for Whisper and the cleanup model, and find→replace rules.

use crate::store::Rule;

/// Whisper's prompt is limited (~224 tokens); keep the hint comfortably below that.
const WHISPER_PROMPT_CHARS: usize = 600;

/// Spelling hint for Whisper: the terms as a comma-separated list, trimmed to the budget.
/// Whisper copies spellings it has seen in its prompt.
pub fn whisper_prompt(terms: &[String]) -> String {
    let mut out = String::new();
    for t in terms {
        let add = if out.is_empty() { t.len() } else { t.len() + 2 };
        if out.len() + add > WHISPER_PROMPT_CHARS {
            break;
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(t);
    }
    if out.is_empty() {
        out
    } else {
        format!("{out}.")
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '\'' || c == '_'
}

/// Apply find→replace rules: case-insensitive, whole words/phrases only, longest phrase first.
pub fn apply_rules(text: &str, rules: &[Rule]) -> String {
    let mut rules: Vec<&Rule> = rules.iter().filter(|r| !r.find.trim().is_empty()).collect();
    rules.sort_by_key(|r| std::cmp::Reverse(r.find.chars().count()));
    let mut out = text.to_string();
    for r in rules {
        out = replace_phrase(&out, r.find.trim(), &r.replace);
    }
    out
}

fn replace_phrase(text: &str, find: &str, replace: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let needle: Vec<char> = find.chars().flat_map(char::to_lowercase).collect();
    // Lower-casing can change length for a few scripts; fall back to no-op rather than mis-slice.
    if lower.len() != chars.len() || needle.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let end = i + needle.len();
        let matches = end <= chars.len()
            && lower[i..end] == needle[..]
            && (i == 0 || !is_word_char(chars[i - 1]))
            && (end == chars.len() || !is_word_char(chars[end]));
        if matches {
            out.push_str(replace);
            i = end;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(find: &str, replace: &str) -> Rule {
        Rule { id: 0, find: find.into(), replace: replace.into() }
    }

    #[test]
    fn prompt_lists_terms_within_budget() {
        assert_eq!(whisper_prompt(&[]), "");
        assert_eq!(whisper_prompt(&["Kubernetes".into(), "Anneliese".into()]), "Kubernetes, Anneliese.");
        let many: Vec<String> = (0..200).map(|i| format!("Term{i}")).collect();
        assert!(whisper_prompt(&many).len() <= WHISPER_PROMPT_CHARS + 1);
    }

    #[test]
    fn rules_match_whole_phrases_case_insensitively() {
        let rules = vec![rule("my email", "me@example.com"), rule("cat", "dog")];
        assert_eq!(apply_rules("Send it to my email, please.", &rules), "Send it to me@example.com, please.");
        assert_eq!(apply_rules("My Email is here", &rules), "me@example.com is here");
        assert_eq!(apply_rules("The category has a cat.", &rules), "The category has a dog.");
        assert_eq!(apply_rules("cat's toy", &rules), "cat's toy", "apostrophe counts as part of the word");
    }

    #[test]
    fn longest_rule_wins() {
        let rules = vec![rule("new york", "NY"), rule("new york city", "NYC")];
        assert_eq!(apply_rules("I love New York City and New York.", &rules), "I love NYC and NY.");
    }
}
