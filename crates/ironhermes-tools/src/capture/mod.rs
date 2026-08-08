// Phase 36.17.8 — Mic capture module (cpal-backed).
//
// Plan 05 fills this module with:
//   - `cpal_capture` submodule (capture_utterance + build_input_stream + write_wav)
//   - `run_conversation_loop` — continuous VAD loop for CLI/TUI voice mode
//
// Consumed by:
//   - crates/ironhermes-cli/src/tui_rata/app.rs (Ctrl+B arm → handle_record_key)
//   - crates/ironhermes-core/src/commands/handlers.rs (cmd_voice on/off)

pub mod cpal_capture;

use std::path::Path;
use std::sync::Arc;

use crate::hallucination_filter::HallucinationFilter;
use crate::vad::VadSettings;
use ironhermes_core::SttProvider;

/// Maximum number of consecutive no-speech cycles before the conversation
/// loop exits automatically (D-08: 3 consecutive no-speech cycles → exit).
const MAX_NO_SPEECH_CYCLES: u32 = 3;

/// Phase reported by the conversation loop so the UI status pill can reflect
/// whether the loop is currently capturing audio or waiting on the STT request.
///
/// Defined here (not in the CLI's `voice_state`) so `run_conversation_loop` has
/// no dependency on the consuming crate; the caller maps it to its own pill enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturePhase {
    /// cpal stream open, EnergyVad running — `● LISTENING`.
    Listening,
    /// WAV captured, `provider.transcribe()` in flight — `◐ TRANSCRIBING`.
    Transcribing,
}

/// Run the continuous VAD conversation loop (D-08).
///
/// Sequence per iteration:
/// 1. `capture_utterance` → `Some(wav_path)` or `None` (no speech).
/// 2. On `Some(wav_path)`:
///    a. Transcribe via `provider.transcribe(&wav_path)`.
///    b. Apply `HallucinationFilter::filter` — drop `None`.
///    c. Call `on_transcript(transcript)` to submit as-if-typed.
///    d. Auto-restart (no-speech counter reset).
/// 3. On `None` (no speech / HardStop): increment no-speech counter.
///    After `MAX_NO_SPEECH_CYCLES` consecutive no-speech cycles, return.
/// 4. If `stop` fires (`true`), exit immediately.
///
/// The `on_transcript` callback receives the filtered transcript and is
/// responsible for injecting it into the chat (e.g. App::submit_voice_text).
/// The callback is `Send + 'static` so it can be invoked from the async task.
///
/// `home` — `$IRONHERMES_HOME` path for WAV output (T-36.17.8-path).
/// `provider` — selected SttProvider (from `build_stt_registry` + `select_stt_provider`).
/// `stop` — watch channel; set to `true` to cancel the loop early.
/// `max_recording_secs` — per-utterance cap (T-36.17.8-cost, default 120 s).
/// `on_transcript` — callback invoked with each accepted transcript.
pub async fn run_conversation_loop(
    home: &Path,
    provider: Arc<dyn SttProvider>,
    stop: tokio::sync::watch::Receiver<bool>,
    max_recording_secs: u32,
    vad_settings: VadSettings,
    on_transcript: impl Fn(String) + Send + 'static,
    on_phase: impl Fn(CapturePhase) + Send + 'static,
) {
    let mut no_speech_cycles: u32 = 0;

    loop {
        // Check stop signal before each capture attempt.
        if *stop.borrow() {
            tracing::debug!("run_conversation_loop: stop signal received");
            return;
        }

        // Entering capture — drive the pill back to LISTENING.
        on_phase(CapturePhase::Listening);

        // Clone stop receiver for this utterance's capture.
        let stop_rx = stop.clone();

        tracing::debug!(
            "run_conversation_loop: starting capture (no_speech_cycles={no_speech_cycles})"
        );

        match cpal_capture::capture_utterance(home, stop_rx, max_recording_secs, vad_settings).await
        {
            Err(e) => {
                // Device/stream errors — log and treat as a no-speech cycle.
                tracing::error!("capture_utterance error: {e:#}");
                no_speech_cycles += 1;
            }
            Ok(None) => {
                // HardStop or stop signal — no speech detected.
                no_speech_cycles += 1;
                tracing::debug!(
                    "run_conversation_loop: no speech (cycle {no_speech_cycles}/{MAX_NO_SPEECH_CYCLES})"
                );
            }
            Ok(Some(wav_path)) => {
                // Reset no-speech counter on any captured audio.
                no_speech_cycles = 0;

                // WAV captured — STT request now in flight; show TRANSCRIBING.
                on_phase(CapturePhase::Transcribing);

                // Transcribe via selected STT provider.
                match provider.transcribe(&wav_path).await {
                    Err(e) => {
                        tracing::error!("transcribe error: {e:#}");
                        // Remove temp WAV on error.
                        let _ = std::fs::remove_file(&wav_path);
                    }
                    Ok(transcript) => {
                        // Remove temp WAV after transcription.
                        let _ = std::fs::remove_file(&wav_path);

                        // Apply hallucination filter (T-36.17.8-phantom).
                        match HallucinationFilter::filter(&transcript) {
                            None => {
                                tracing::debug!(
                                    "run_conversation_loop: hallucination filtered: {:?}",
                                    transcript
                                );
                                // Filtered transcript = a no-speech cycle (no submission).
                                no_speech_cycles += 1;
                            }
                            Some(accepted) => {
                                tracing::info!(
                                    "run_conversation_loop: transcript accepted: {:?}",
                                    accepted
                                );
                                on_transcript(accepted.to_owned());
                                no_speech_cycles = 0;
                            }
                        }
                    }
                }
            }
        }

        // D-08: exit after MAX_NO_SPEECH_CYCLES consecutive no-speech cycles.
        if no_speech_cycles >= MAX_NO_SPEECH_CYCLES {
            tracing::info!(
                "run_conversation_loop: {} consecutive no-speech cycles — exiting",
                MAX_NO_SPEECH_CYCLES
            );
            return;
        }

        // Check stop signal at end of each cycle too.
        if *stop.borrow() {
            return;
        }
    }
}
