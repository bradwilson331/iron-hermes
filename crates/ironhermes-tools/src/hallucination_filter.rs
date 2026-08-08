// Phase 36.17.8 — STT hallucination filter (D-12).
// Canonical phrase list confirmed against hermes-agent tools/voice_mode.py
// WHISPER_HALLUCINATIONS by the orchestrator (Task 1 pre-resolved, 2026-06-09).

/// Known Whisper hallucination phrases (multi-language, 26 entries).
///
/// Sourced from hermes-agent `tools/voice_mode.py` WHISPER_HALLUCINATIONS,
/// confirmed by operator (Task 1 pre-resolved). Stored lowercased for
/// case-insensitive exact-match comparison.
///
/// D-12: applied post-transcribe, pre-agent on every transcript.
pub static HALLUCINATION_PHRASES: &[&str] = &[
    "thank you.",
    "thank you",
    "thanks for watching.",
    "thanks for watching",
    "subscribe to my channel.",
    "subscribe to my channel",
    "like and subscribe.",
    "like and subscribe",
    "please subscribe.",
    "please subscribe",
    "thank you for watching.",
    "thank you for watching",
    "bye.",
    "bye",
    "you",
    "the end.",
    "the end",
    "продолжение следует",
    "продолжение следует...",
    "sous-titres",
    "sous-titres réalisés par la communauté d'amara.org",
    "sottotitoli creati dalla comunità amara.org",
    "untertitel von stephanie geiges",
    "amara.org",
    "www.mooji.org",
    "ご視聴ありがとうございました",
];

/// Number of consecutive identical words that marks a repetition hallucination.
///
/// Mirrors the canonical Python regex `(\w{2,})\s+(\1\s*){3,}` — one word plus
/// three or more repeats = four occurrences. Rust's `regex` crate has no
/// backreference support, so the equivalent check is done in plain Rust.
const REPETITION_RUN_THRESHOLD: usize = 4;

/// Returns `true` if `cleaned` contains a word (≥2 alphanumeric chars) repeated
/// `REPETITION_RUN_THRESHOLD`+ times consecutively, e.g. "yes yes yes yes".
///
/// Plain-Rust replacement for the backreference regex `(\w{2,})\s+(\1\s*){3,}`,
/// which the `regex` crate cannot compile (no backreferences). `cleaned` is
/// already trimmed + lowercased by `filter`.
fn has_repetition_run(cleaned: &str) -> bool {
    let mut prev: Option<&str> = None;
    let mut run = 1usize;
    for raw in cleaned.split_whitespace() {
        // Strip surrounding punctuation; compare the bare word token.
        let word = raw.trim_matches(|c: char| !c.is_alphanumeric());
        if word.chars().count() < 2 {
            prev = None;
            run = 1;
            continue;
        }
        if prev == Some(word) {
            run += 1;
            if run >= REPETITION_RUN_THRESHOLD {
                return true;
            }
        } else {
            prev = Some(word);
            run = 1;
        }
    }
    false
}

/// Filters known STT hallucination phrases and repetition patterns.
///
/// This is a plain struct with a static method — not a trait, not a tool.
/// Shared between the CLI capture loop (Plan 05) and the web STT path (Plan 06).
pub struct HallucinationFilter;

impl HallucinationFilter {
    /// Returns `None` if `transcript` is a hallucination, `Some(transcript)` otherwise.
    ///
    /// Match semantics (approved by operator, Task 1):
    /// 1. Trim + lowercase the transcript.
    /// 2. Empty after trim → `None` (reject).
    /// 3. Exact-match against `HALLUCINATION_PHRASES`: reject if the phrase set
    ///    contains `cleaned` **or** `cleaned` with trailing `.`/`!` stripped.
    ///    (Mirrors Python `cleaned.rstrip('.!') in SET or cleaned in SET`.)
    /// 4. Generic repetition check (any word repeated 4+ times) → `None`.
    /// 5. Otherwise → `Some(transcript)` (original, untrimmed).
    pub fn filter(transcript: &str) -> Option<&str> {
        let cleaned = transcript.trim().to_lowercase();
        if cleaned.is_empty() {
            return None;
        }

        // Step 3 — exact match (with and without trailing punctuation).
        let stripped = cleaned.trim_end_matches(['.', '!']);
        for &phrase in HALLUCINATION_PHRASES {
            if cleaned == phrase || stripped == phrase {
                return None;
            }
        }

        // Step 4 — generic repetition check (any word repeated 4+ times).
        if has_repetition_run(&cleaned) {
            return None;
        }

        Some(transcript)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── D-12 test_known_phrase_rejected ──────────────────────────────────────
    // A canonical hallucination phrase must be rejected regardless of case.
    #[test]
    fn test_known_phrase_rejected() {
        // "Thank you for watching" is in the approved phrase list (lowercased).
        assert_eq!(HallucinationFilter::filter("Thank you for watching"), None);
        // Also test with trailing period variant.
        assert_eq!(HallucinationFilter::filter("thank you for watching."), None);
        // Non-English phrase.
        assert_eq!(
            HallucinationFilter::filter("ご視聴ありがとうございました"),
            None
        );
    }

    // ── D-12 test_repetition_rejected ────────────────────────────────────────
    // A word repeated 4+ times must be caught by the generic regex.
    #[test]
    fn test_repetition_rejected() {
        assert_eq!(HallucinationFilter::filter("yes yes yes yes"), None);
        // Also test with more repetitions.
        assert_eq!(HallucinationFilter::filter("ha ha ha ha ha"), None);
    }

    // ── D-12 test_normal_passes ───────────────────────────────────────────────
    // A normal transcript must be returned unchanged (original, not trimmed).
    #[test]
    fn test_normal_passes() {
        let input = "what is the weather today";
        assert_eq!(HallucinationFilter::filter(input), Some(input));
        // Multi-word, mixed case — must preserve original.
        let input2 = "  Hello, how are you doing?  ";
        assert_eq!(HallucinationFilter::filter(input2), Some(input2));
    }
}
