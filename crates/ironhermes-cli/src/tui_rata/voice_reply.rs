// Phase 36.17.8 — spoken agent replies (auto_tts wiring).
//
// `/voice` exposes three states (per the UX spec):
//   - `/voice off`  → never speak.
//   - `/voice on`   → speak the reply ONLY when the turn's input was voice.
//   - `/voice tts`  → speak EVERY reply (typed or voice).
//
// This module owns the deterministic post-turn speak path: after `run_turn`
// returns the assistant's `final_response`, `spawn_turn` consults
// [`should_speak`], cleans the text with [`spoken_text`], and plays it via
// [`speak_reply`]. This is separate from the agent-callable `text_to_speech` /
// `send_audio` tools — auto_tts speaks on every (qualifying) turn without the
// model having to choose to.

use std::path::Path;
use std::sync::Arc;

use ironhermes_core::Config;

/// Decide whether this turn's reply should be spoken.
///
/// - `auto_tts` (`/voice tts`) speaks every reply.
/// - `enabled` (`/voice on`) speaks only when the turn's input was voice.
///
/// `/voice off` leaves both `false`, so nothing is spoken.
pub fn should_speak(auto_tts: bool, enabled: bool, turn_was_voice: bool) -> bool {
    auto_tts || (enabled && turn_was_voice)
}

/// Maximum characters of spoken text. Long replies (and code dumps) are
/// truncated so a single turn does not narrate for minutes. Chosen to comfortably
/// hold a few sentences of normal prose.
const MAX_SPOKEN_CHARS: usize = 600;

/// Convert a markdown agent reply into listenable plain text.
///
/// Strips content that is meaningless or grating when read aloud:
/// - fenced code blocks (``` … ```) — replaced with a short spoken placeholder
/// - inline `code` spans — backticks removed, text kept
/// - markdown emphasis/heading/list/blockquote markers
/// - link syntax `[text](url)` — keeps `text`, drops the URL
///
/// Collapses the result to single-spaced text and caps it at
/// [`MAX_SPOKEN_CHARS`] (truncating on a word boundary when possible).
pub fn spoken_text(reply: &str) -> String {
    // 1. Drop fenced code blocks wholesale. Toggle on ``` lines.
    let mut in_fence = false;
    let mut had_code = false;
    let mut kept: Vec<String> = Vec::new();
    for line in reply.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            had_code = true;
            continue;
        }
        if in_fence {
            continue;
        }
        kept.push(strip_inline_markup(line));
    }
    let mut text = kept.join(" ");
    if had_code {
        // A single spoken cue rather than reading code aloud.
        text.push_str(" (code omitted)");
    }

    // 2. Collapse runs of whitespace to single spaces.
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");

    // 3. Cap length on a word boundary.
    truncate_on_word(&collapsed, MAX_SPOKEN_CHARS)
}

/// Strip inline markdown markup from a single line, keeping the prose.
fn strip_inline_markup(line: &str) -> String {
    let mut s = line.to_string();

    // Leading heading / list / blockquote markers.
    let trimmed = s.trim_start();
    let lead_stripped = trimmed
        .trim_start_matches('#')
        .trim_start_matches('>')
        .trim_start_matches(['-', '*', '+'])
        .trim_start();
    s = lead_stripped.to_string();

    // Link syntax [text](url) → text. No regex (project ships none here); a
    // small manual scan keeps `text` and drops the parenthesised target.
    s = collapse_links(&s);

    // Drop the remaining emphasis / code markup characters.
    s.chars()
        .filter(|c| !matches!(c, '`' | '*' | '_' | '#' | '~'))
        .collect()
}

/// Replace `[text](url)` with `text`. Leaves non-link brackets untouched.
///
/// Char-boundary safe: scans with `str::find` (byte offsets at ASCII delimiters)
/// and slices only at those boundaries, so non-ASCII prose passes through intact.
fn collapse_links(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after_open = &rest[open + 1..];
        if let Some(close_rel) = after_open.find(']') {
            let text = &after_open[..close_rel];
            let after_close = &after_open[close_rel + 1..];
            if after_close.starts_with('(')
                && let Some(paren_rel) = after_close.find(')')
            {
                // It's a link: keep `text`, drop the parenthesised target.
                out.push_str(text);
                rest = &after_close[paren_rel + 1..];
                continue;
            }
        }
        // Not a link: emit the literal '[' and continue past it.
        out.push('[');
        rest = after_open;
    }
    out.push_str(rest);
    out
}

/// Truncate `s` to at most `max` chars, preferring the last word boundary so a
/// word is not cut mid-syllable. Appends `…` when truncated.
fn truncate_on_word(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let prefix: String = s.chars().take(max).collect();
    match prefix.rfind(' ') {
        Some(idx) if idx > max / 2 => format!("{}…", &prefix[..idx]),
        _ => format!("{prefix}…"),
    }
}

/// Synthesize `text` with the configured TTS provider and play it locally.
///
/// Provider selection follows `config.tts.provider` (default `"edge"`, keyless)
/// and falls back to Edge when the configured provider is unavailable (e.g.
/// ElevenLabs without an API key). Playback reuses the shipped `SendAudioTool`
/// `Platform::Local` rodio arm (D-15) so there is a single audio-output path.
///
/// Never panics and never returns an error: every failure (no provider, synth
/// error, no audio device) is logged at `warn` and swallowed — a missing voice
/// must not break the text turn that already completed.
pub async fn speak_reply(config: Arc<Config>, home: &Path, text: &str) {
    use ironhermes_core::{SessionKey, types::Platform};
    use ironhermes_tools::registry::Tool;
    use ironhermes_tools::send_audio_tool::SendAudioTool;

    // Select provider: configured-if-available, else Edge fallback.
    let registry = ironhermes_tools::tts::build_tts_registry(&config.tts);
    let provider = registry
        .get(&config.tts.provider)
        .filter(|p| p.is_available())
        .or_else(|| registry.get("edge"));
    let Some(provider) = provider else {
        tracing::warn!("speak_reply: no TTS provider available; skipping spoken reply");
        return;
    };

    // Synthesize into the audio cache (D-16: $IRONHERMES_HOME/audio_cache/<uuid>).
    let audio_dir = home.join("audio_cache");
    if let Err(e) = std::fs::create_dir_all(&audio_dir) {
        tracing::warn!("speak_reply: cannot create audio_cache dir: {e}");
        return;
    }
    let out_path = audio_dir.join(format!("{}.mp3", uuid::Uuid::new_v4()));
    let written = match provider.synthesize(text, &out_path).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("speak_reply: synthesize failed: {e:#}");
            return;
        }
    };

    // Play through the shipped Local rodio arm (mirrors `hermes tts play`).
    let key = SessionKey::new(Platform::Local, "voice-reply");
    let tool = SendAudioTool::new(key, None, config);
    let path_arg = written.to_string_lossy().to_string();
    if let Err(e) = tool.execute(serde_json::json!({ "path": path_arg })).await {
        tracing::warn!("speak_reply: playback failed: {e:#}");
    }

    // Best-effort cleanup of the synthesized file.
    let _ = std::fs::remove_file(&written);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_speak_matches_three_voice_states() {
        // /voice off — both flags false → silent.
        assert!(!should_speak(false, false, true));
        assert!(!should_speak(false, false, false));
        // /voice on — speak only voice-input turns.
        assert!(should_speak(false, true, true));
        assert!(!should_speak(false, true, false));
        // /voice tts — speak every reply regardless of input source.
        assert!(should_speak(true, false, false));
        assert!(should_speak(true, true, false));
    }

    #[test]
    fn spoken_text_strips_code_fences() {
        let reply = "Here is the fix:\n```rust\nfn main() {}\n```\nThat should work.";
        let spoken = spoken_text(reply);
        assert!(
            !spoken.contains("fn main"),
            "code body must be dropped: {spoken}"
        );
        assert!(spoken.contains("Here is the fix"));
        assert!(spoken.contains("That should work"));
        assert!(spoken.contains("(code omitted)"));
    }

    #[test]
    fn spoken_text_strips_inline_markup_and_links() {
        let reply = "## Heading\n- **bold** and `code` and a [link](https://example.com) here";
        let spoken = spoken_text(reply);
        assert!(!spoken.contains('#'));
        assert!(!spoken.contains('*'));
        assert!(!spoken.contains('`'));
        assert!(
            !spoken.contains("https://"),
            "url must be dropped: {spoken}"
        );
        assert!(spoken.contains("link"), "link text must remain: {spoken}");
        assert!(spoken.contains("bold"));
        assert!(spoken.contains("Heading"));
    }

    #[test]
    fn spoken_text_caps_length_on_word_boundary() {
        let long = "word ".repeat(400); // ~2000 chars
        let spoken = spoken_text(&long);
        assert!(
            spoken.chars().count() <= MAX_SPOKEN_CHARS + 1,
            "len={}",
            spoken.chars().count()
        );
        assert!(spoken.ends_with('…'));
        // Truncation lands on a boundary — no partial "wor" fragment at the end.
        assert!(spoken.trim_end_matches('…').ends_with("word"));
    }

    #[test]
    fn spoken_text_plain_prose_passes_through() {
        let spoken = spoken_text("Hello, can you hear me?");
        assert_eq!(spoken, "Hello, can you hear me?");
    }
}
