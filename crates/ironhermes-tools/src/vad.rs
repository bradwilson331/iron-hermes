// Phase 36.17.8 — Voice Activity Detection.
// Plan 04 implements EnergyVad state machine (D-09).

/// Decision returned by `EnergyVad` after processing each audio frame.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VadDecision {
    /// Speech is ongoing — keep capturing.
    Continue,
    /// End-of-speech silence window elapsed — send to STT.
    EndOfSpeech,
    /// Hard-stop threshold reached (no speech for ~15 s).
    HardStop,
}

// D-09 constants (CONTEXT authoritative; use 0.5 s speech-confirm, not voice-mode.txt's 0.3 s).
/// Default RMS energy below which audio counts as silence. Overridable per-mic via
/// `voice.silence_threshold` — some built-in mics (e.g. MacBook at 48 kHz F32) run
/// well below 200 even during speech, so a fixed threshold blocks end-of-speech.
pub const DEFAULT_SILENCE_RMS_THRESHOLD: i32 = 200;
/// Default seconds of continuous silence that ends a recording. Overridable via
/// `voice.silence_duration`.
pub const DEFAULT_SILENCE_DURATION_SECS: f64 = 3.0;
const SPEECH_CONFIRM_SECS: f64 = 0.5;
const NO_SPEECH_HARD_STOP_SECS: f64 = 15.0;
const SAMPLE_RATE: u32 = 16_000;

/// Tunable VAD thresholds sourced from `VoiceConfig` (`voice.silence_threshold` /
/// `voice.silence_duration`). `speech_confirm` (0.5 s) and the 15 s hard-stop stay fixed.
#[derive(Debug, Clone, Copy)]
pub struct VadSettings {
    /// RMS below which a chunk is silence (D-09 `voice.silence_threshold`).
    pub silence_threshold: i32,
    /// Seconds of silence after confirmed speech that ends a recording.
    pub silence_duration_secs: f64,
}

impl Default for VadSettings {
    fn default() -> Self {
        Self {
            silence_threshold: DEFAULT_SILENCE_RMS_THRESHOLD,
            silence_duration_secs: DEFAULT_SILENCE_DURATION_SECS,
        }
    }
}

/// Energy-based Voice Activity Detector (D-09).
///
/// Feed audio chunks via [`push_chunk`] — it returns a [`VadDecision`] after
/// each chunk. The detector never opens an audio device; it only processes
/// pre-captured `i16` samples. This makes it fully unit-testable with
/// synthetic buffers (RESEARCH Pitfall 3 / macOS TCC).
///
/// State machine:
/// - Accumulates RMS energy per chunk.
/// - Arms `speech_confirmed` once cumulative above-threshold time ≥ 0.5 s.
/// - Returns `EndOfSpeech` when speech was confirmed AND silence ≥ 3.0 s.
/// - Returns `HardStop` when total time ≥ 15.0 s with no speech confirmed.
/// - Returns `Continue` otherwise.
#[derive(Debug)]
pub struct EnergyVad {
    /// Set to true once cumulative above-threshold samples reach SPEECH_CONFIRM_SECS.
    speech_confirmed: bool,
    /// Cumulative samples where RMS ≥ threshold (reset to 0 on silence chunk).
    speech_samples: u64,
    /// Cumulative samples where RMS < threshold (reset on speech chunk).
    silence_samples: u64,
    /// Total samples fed so far (monotonically increasing).
    total_samples: u64,
    /// Tunable thresholds (silence RMS + silence duration).
    settings: VadSettings,
}

impl EnergyVad {
    /// Create a new, zeroed detector using the default D-09 thresholds.
    pub fn new() -> Self {
        Self::with_settings(VadSettings::default())
    }

    /// Create a detector with caller-supplied thresholds (from `VoiceConfig`).
    pub fn with_settings(settings: VadSettings) -> Self {
        Self {
            speech_confirmed: false,
            speech_samples: 0,
            silence_samples: 0,
            total_samples: 0,
            settings,
        }
    }

    /// Feed a chunk of i16 samples. Returns a [`VadDecision`].
    ///
    /// Chunk size is irrelevant — the counters track cumulative time so
    /// any chunk size (512, 1024, 4096 …) works correctly.
    pub fn push_chunk(&mut self, samples: &[i16]) -> VadDecision {
        let rms = Self::rms(samples);
        let n = samples.len() as u64;
        self.total_samples += n;

        if rms >= self.settings.silence_threshold as f64 {
            // Above-threshold: accumulate speech, reset silence counter.
            self.speech_samples += n;
            self.silence_samples = 0;

            if !self.speech_confirmed
                && self.speech_samples >= (SPEECH_CONFIRM_SECS * SAMPLE_RATE as f64) as u64
            {
                self.speech_confirmed = true;
            }
            VadDecision::Continue
        } else {
            // Below-threshold: accumulate silence.
            self.silence_samples += n;
            let silence_secs = self.silence_samples as f64 / SAMPLE_RATE as f64;
            let total_secs = self.total_samples as f64 / SAMPLE_RATE as f64;

            if self.speech_confirmed && silence_secs >= self.settings.silence_duration_secs {
                VadDecision::EndOfSpeech
            } else if total_secs >= NO_SPEECH_HARD_STOP_SECS {
                VadDecision::HardStop
            } else {
                VadDecision::Continue
            }
        }
    }

    /// Root-mean-square energy of a sample slice.
    fn rms(samples: &[i16]) -> f64 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f64 = samples.iter().map(|&s| (s as f64).powi(2)).sum();
        (sum_sq / samples.len() as f64).sqrt()
    }
}

impl Default for EnergyVad {
    fn default() -> Self {
        Self::new()
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Build N samples at a constant amplitude (all samples set to `amplitude`).
#[cfg(test)]
fn make_samples(n: usize, amplitude: i16) -> Vec<i16> {
    vec![amplitude; n]
}

/// Samples needed to represent `secs` seconds at 16 kHz.
#[cfg(test)]
fn samples_for_secs(secs: f64) -> usize {
    (secs * SAMPLE_RATE as f64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    // UAT regression: a quiet mic (rms 100, below the default 200) must still be
    // usable by lowering `voice.silence_threshold`. With the default threshold the
    // same audio never confirms speech (which left the capture loop stuck on a
    // MacBook mic until the 15 s hard-stop).
    #[test]
    fn test_configurable_threshold_confirms_quiet_speech() {
        let quiet = make_samples(samples_for_secs(0.6), 100); // rms = 100

        // Default threshold 200 → rms 100 is "silence", speech never confirms.
        let mut def = EnergyVad::new();
        assert_eq!(def.push_chunk(&quiet), VadDecision::Continue);
        assert!(
            !def.speech_confirmed,
            "rms 100 must NOT confirm speech at the default threshold 200"
        );

        // Lowered threshold 50 → rms 100 now registers as speech, and 3 s of
        // silence afterwards yields EndOfSpeech.
        let mut low = EnergyVad::with_settings(VadSettings {
            silence_threshold: 50,
            silence_duration_secs: 3.0,
        });
        assert_eq!(low.push_chunk(&quiet), VadDecision::Continue);
        assert!(
            low.speech_confirmed,
            "rms 100 must confirm speech at threshold 50"
        );
        let silence = make_samples(samples_for_secs(3.0), 0);
        assert_eq!(low.push_chunk(&silence), VadDecision::EndOfSpeech);
    }

    // ── D-09 test_speech_confirm_arm ─────────────────────────────────────────
    // After cumulative above-threshold samples ≥ 0.5 s, speech_confirmed is armed.
    // Before that threshold, a single silent chunk must not set it.
    #[test]
    fn test_speech_confirm_arm() {
        let mut vad = EnergyVad::new();

        // Feed exactly 0.4 s of above-threshold audio (< SPEECH_CONFIRM_SECS = 0.5 s).
        let short_samples = make_samples(samples_for_secs(0.4), 500);
        let decision = vad.push_chunk(&short_samples);
        // Still within confirm window → Continue, not yet confirmed.
        assert_eq!(decision, VadDecision::Continue);
        assert!(!vad.speech_confirmed, "0.4 s should not yet confirm speech");

        // Now feed another 0.2 s (total 0.6 s ≥ 0.5 s) — should arm confirmed.
        let top_up = make_samples(samples_for_secs(0.2), 500);
        let decision2 = vad.push_chunk(&top_up);
        assert_eq!(decision2, VadDecision::Continue);
        assert!(
            vad.speech_confirmed,
            "0.6 s cumulative should confirm speech"
        );
    }

    // ── D-09 test_end_of_speech ──────────────────────────────────────────────
    // After speech is confirmed, 3.0 s of silence must return EndOfSpeech.
    #[test]
    fn test_end_of_speech() {
        let mut vad = EnergyVad::new();

        // Confirm speech (0.6 s above threshold).
        vad.push_chunk(&make_samples(samples_for_secs(0.6), 500));
        assert!(vad.speech_confirmed);

        // Feed 2.9 s of silence (just under 3.0 s threshold) → still Continue.
        let result = vad.push_chunk(&make_samples(samples_for_secs(2.9), 0));
        assert_eq!(
            result,
            VadDecision::Continue,
            "2.9 s silence should still be Continue"
        );

        // Feed another 0.2 s of silence (total ≥ 3.0 s) → EndOfSpeech.
        let result2 = vad.push_chunk(&make_samples(samples_for_secs(0.2), 0));
        assert_eq!(result2, VadDecision::EndOfSpeech);
    }

    // ── D-09 test_hard_stop ──────────────────────────────────────────────────
    // 15 s of below-threshold with NO speech → HardStop.
    #[test]
    fn test_hard_stop() {
        let mut vad = EnergyVad::new();

        // Feed 14.9 s of silence (no speech ever) — must not hard-stop yet.
        let result = vad.push_chunk(&make_samples(samples_for_secs(14.9), 0));
        assert_eq!(
            result,
            VadDecision::Continue,
            "14.9 s should still be Continue"
        );

        // Feed another 0.2 s → total ≥ 15.0 s → HardStop.
        let result2 = vad.push_chunk(&make_samples(samples_for_secs(0.2), 0));
        assert_eq!(result2, VadDecision::HardStop);
    }

    // ── D-09 test_no_early_stop ──────────────────────────────────────────────
    // 3.0 s of silence BEFORE any speech confirmation must NOT return EndOfSpeech.
    #[test]
    fn test_no_early_stop() {
        let mut vad = EnergyVad::new();

        // Feed 3.1 s of silence with no speech at all.
        let result = vad.push_chunk(&make_samples(samples_for_secs(3.1), 0));
        // speech_confirmed is still false → EndOfSpeech must not fire.
        assert_ne!(
            result,
            VadDecision::EndOfSpeech,
            "EndOfSpeech must not fire without prior speech confirmation"
        );
        // It should be Continue (total < 15 s).
        assert_eq!(result, VadDecision::Continue);
    }
}
