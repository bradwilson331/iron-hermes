// Phase 36.17.8 — cpal mic capture + EnergyVad-driven WAV write.
//
// Threat mitigations (from plan threat model):
//   T-36.17.8-cost  — hard cap at voice.max_recording_seconds; WAV size bounded before transcribe
//   T-36.17.8-panic — missing device returns clean anyhow::Err, never panic
//   T-36.17.8-path  — WAV filename is uuid-generated; no user-controlled path component (D-19)
//
// Pitfalls avoided (RESEARCH §Pitfalls 1/2/3):
//   1. Pitfall 1 — device native rate negotiation; fallback to nearest + linear resample
//   2. Pitfall 2 — cpal callback is SYNC; std::sync::Mutex only; never .await inside
//   3. Pitfall 3 — never open a real audio device in unit tests (macOS TCC)

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Stream, StreamConfig, SupportedStreamConfig};
use hound::{SampleFormat, WavSpec, WavWriter};
use uuid::Uuid;

use crate::vad::{EnergyVad, VadDecision, VadSettings};

/// Target sample rate for Whisper STT (D-09 / RESEARCH Pitfall 1).
/// In cpal 0.18, SampleRate is a type alias for u32.
const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Samples per periodic VAD drain tick (10 ms at 16 kHz = 160 samples).
const VAD_TICK_SAMPLES: usize = 160;

/// VAD tick interval — match the expected chunk size.
const VAD_TICK_INTERVAL: Duration = Duration::from_millis(10);

// ── Linear resampler (inline — rubato not in workspace) ───────────────────────

/// Linear-interpolation resampler: converts `samples` from `src_rate` to 16 kHz.
///
/// Simple quality resample suitable for speech (Whisper is robust).
/// Only invoked when the device does not support 16 kHz natively (Pitfall 1).
fn linear_resample(samples: &[i16], src_rate: u32) -> Vec<i16> {
    if src_rate == TARGET_SAMPLE_RATE || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = src_rate as f64 / TARGET_SAMPLE_RATE as f64;
    let out_len = ((samples.len() as f64) / ratio).ceil() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let src_idx = src_pos as usize;
        let frac = src_pos - src_idx as f64;
        let s0 = *samples.get(src_idx).unwrap_or(&0) as f64;
        let s1 = *samples.get(src_idx + 1).unwrap_or(&0) as f64;
        out.push((s0 + frac * (s1 - s0)) as i16);
    }
    out
}

// ── WAV writing ───────────────────────────────────────────────────────────────

/// Write `samples` (16 kHz mono i16) to a UUID-named WAV under
/// `$IRONHERMES_HOME/audio_cache/`. Returns the file path on success.
///
/// T-36.17.8-path: filename is UUID-generated; no user-controlled component.
pub fn write_wav(audio_dir: &Path, samples: &[i16]) -> Result<PathBuf> {
    std::fs::create_dir_all(audio_dir)
        .with_context(|| format!("create audio_cache dir {:?}", audio_dir))?;

    let wav_path = audio_dir.join(format!("{}.wav", Uuid::new_v4()));
    let spec = WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(&wav_path, spec)
        .with_context(|| format!("create WavWriter at {:?}", wav_path))?;
    for &s in samples {
        writer.write_sample(s)?;
    }
    writer.finalize()?;
    Ok(wav_path)
}

// ── Capture pipeline ─────────────────────────────────────────────────────────

/// Negotiate the best input config for the given device.
///
/// Strategy (RESEARCH Pitfall 1):
/// 1. Try to find a config range that supports 16 kHz.
/// 2. If unavailable, accept the device default config and resample later.
///
/// Returns `(SupportedStreamConfig, actual_capture_sample_rate)`.
fn negotiate_config(device: &cpal::Device) -> Result<(SupportedStreamConfig, u32)> {
    // Walk supported configs looking for one whose range covers 16 kHz.
    if let Ok(supported) = device.supported_input_configs() {
        for range in supported {
            let min = range.min_sample_rate();
            let max = range.max_sample_rate();
            if min <= TARGET_SAMPLE_RATE && TARGET_SAMPLE_RATE <= max {
                // try_with_sample_rate returns Option<SupportedStreamConfig> in cpal 0.18.
                if let Some(cfg) = range.try_with_sample_rate(TARGET_SAMPLE_RATE) {
                    return Ok((cfg, TARGET_SAMPLE_RATE));
                }
            }
        }
    }
    // Fall back to device default.
    let cfg = device
        .default_input_config()
        .context("no default input config")?;
    let native_rate = cfg.sample_rate();
    Ok((cfg, native_rate))
}

/// Build a cpal input stream that fills `shared_buf` with mono i16 samples.
///
/// The cpal callback is SYNC — only `std::sync::Mutex` is used here
/// (RESEARCH Pitfall 2 — NEVER .await inside the callback).
///
/// Returns the `Stream` (caller must keep alive for the duration of capture).
pub fn build_input_stream(
    device: &cpal::Device,
    supported: &SupportedStreamConfig,
    shared_buf: Arc<Mutex<Vec<i16>>>,
) -> Result<Stream> {
    use cpal::SampleFormat;

    let sample_format = supported.sample_format();
    // Convert SupportedStreamConfig → StreamConfig (cpal 0.18 API).
    let stream_config: StreamConfig = (*supported).into();
    let channels = stream_config.channels as usize;

    // Error callback — log at warn level (lands in tui.log per event_loop wiring).
    let err_fn = |err| {
        tracing::warn!("cpal input stream error: {err}");
    };

    // Build stream: downmix to mono, convert to i16, append to shared buffer.
    // All closures are SYNC — no async, no tokio primitives.
    let stream = match sample_format {
        SampleFormat::I16 => {
            let buf = shared_buf.clone();
            device
                .build_input_stream(
                    stream_config,
                    move |data: &[i16], _| {
                        let mono = downmix_i16(data, channels);
                        if let Ok(mut g) = buf.lock() {
                            g.extend_from_slice(&mono);
                        }
                    },
                    err_fn,
                    None,
                )
                .context("build i16 input stream")?
        }
        SampleFormat::F32 => {
            let buf = shared_buf.clone();
            device
                .build_input_stream(
                    stream_config,
                    move |data: &[f32], _| {
                        let mono = downmix_f32(data, channels);
                        if let Ok(mut g) = buf.lock() {
                            g.extend_from_slice(&mono);
                        }
                    },
                    err_fn,
                    None,
                )
                .context("build f32 input stream")?
        }
        SampleFormat::U8 => {
            let buf = shared_buf.clone();
            device
                .build_input_stream(
                    stream_config,
                    move |data: &[u8], _| {
                        let mono = downmix_u8(data, channels);
                        if let Ok(mut g) = buf.lock() {
                            g.extend_from_slice(&mono);
                        }
                    },
                    err_fn,
                    None,
                )
                .context("build u8 input stream")?
        }
        other => {
            return Err(anyhow!("unsupported cpal sample format: {:?}", other));
        }
    };

    Ok(stream)
}

// ── mono downmix helpers ─────────────────────────────────────────────────────

fn downmix_i16(data: &[i16], channels: usize) -> Vec<i16> {
    if channels == 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|ch| {
            let sum: i32 = ch.iter().map(|&s| s as i32).sum();
            (sum / channels as i32) as i16
        })
        .collect()
}

fn downmix_f32(data: &[f32], channels: usize) -> Vec<i16> {
    if channels == 1 {
        return data
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect();
    }
    data.chunks(channels)
        .map(|ch| {
            let avg: f32 = ch.iter().sum::<f32>() / channels as f32;
            (avg.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
        })
        .collect()
}

fn downmix_u8(data: &[u8], channels: usize) -> Vec<i16> {
    if channels == 1 {
        return data
            .iter()
            .map(|&s| ((s as i32 - 128) * 256) as i16)
            .collect();
    }
    data.chunks(channels)
        .map(|ch| {
            let sum: i32 = ch.iter().map(|&s| (s as i32 - 128) * 256).sum();
            (sum / channels as i32) as i16
        })
        .collect()
}

// ── VAD feeding ────────────────────────────────────────────────────────────────

/// Feed a single freshly-captured chunk to the VAD in 160-sample windows and
/// return the first non-`Continue` decision (or `Continue`).
///
/// `EnergyVad` is a cumulative state machine: each call must receive ONLY the
/// new samples captured since the last tick — never the whole accumulated
/// recording. Feeding the accumulated buffer re-feeds every prior window on each
/// tick, so the VAD's `total_samples`/`silence_samples` grow quadratically and
/// its sense of elapsed time races far ahead of real time (a ~15 s hard-stop
/// fires in well under 1 s). The regression tests below pin this.
fn feed_vad(vad: &mut EnergyVad, chunk_16k: &[i16]) -> VadDecision {
    let mut decision = VadDecision::Continue;
    for window in chunk_16k.chunks(VAD_TICK_SAMPLES) {
        decision = vad.push_chunk(window);
        if decision != VadDecision::Continue {
            break;
        }
    }
    decision
}

// ── manual-stop flush decision ─────────────────────────────────────────────────

/// Decide whether a manually-stopped capture (second Ctrl+B → stop signal) should
/// still be transcribed instead of discarded.
///
/// Push-to-talk semantics: pressing the record key again means "send what I just
/// said", so a half-finished utterance must be flushed — not thrown away. We only
/// discard when nothing above the silence threshold was ever captured (the user
/// stopped before speaking, or only background noise was heard), which avoids
/// shipping an empty/near-silent WAV to the STT provider.
///
/// `max_rms_seen` — loudest chunk RMS observed this utterance.
/// `silence_threshold` — active `voice.silence_threshold` (RMS below = silence).
/// `accumulated_len` — number of 16 kHz samples captured so far.
fn flush_on_stop(max_rms_seen: f64, silence_threshold: i32, accumulated_len: usize) -> bool {
    accumulated_len > 0 && max_rms_seen >= silence_threshold as f64
}

// ── capture_utterance ─────────────────────────────────────────────────────────

/// Capture a single utterance: open device → start stream → run VAD loop →
/// write WAV → return path.
///
/// Returns:
/// - `Ok(Some(path))` — speech captured, VAD ended cleanly, WAV written.
/// - `Ok(None)` — no speech detected (HardStop fired = no-speech cycle).
/// - `Err(...)` — device/stream/IO error.
///
/// `stop` watch receiver lets the caller cancel early (e.g. Ctrl+B while recording).
/// `max_secs` > 0 applies the T-36.17.8-cost per-utterance cap.
pub async fn capture_utterance(
    home: &Path,
    stop: tokio::sync::watch::Receiver<bool>,
    max_secs: u32,
    vad_settings: VadSettings,
) -> Result<Option<PathBuf>> {
    // Open device — clean Err on absence (T-36.17.8-panic: never panic).
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("No input device found"))?;

    tracing::debug!("capture_utterance: opening default input device");

    let (cfg, capture_rate) = negotiate_config(&device)?;
    tracing::info!(
        "capture_utterance: format={:?} capture_rate={capture_rate} channels={}",
        cfg.sample_format(),
        cfg.channels(),
    );
    let shared_buf: Arc<Mutex<Vec<i16>>> = Arc::new(Mutex::new(Vec::new()));
    let stream = build_input_stream(&device, &cfg, shared_buf.clone())?;

    stream.play().context("start cpal input stream")?;

    // Max samples cap at the CAPTURE rate — T-36.17.8-cost.
    let max_capture_samples: u64 = if max_secs > 0 {
        (capture_rate as u64) * (max_secs as u64)
    } else {
        u64::MAX
    };

    let mut vad = EnergyVad::with_settings(vad_settings);
    // Accumulate 16 kHz samples for VAD + eventual WAV write.
    let mut accumulated_16k: Vec<i16> = Vec::new();
    let mut total_capture_samples: u64 = 0;
    // Diagnostics: count ticks and the loudest chunk seen so a RUST_LOG run shows
    // whether audio is flowing and whether it clears the VAD's RMS threshold.
    let mut tick: u64 = 0;
    let mut max_rms_seen: f64 = 0.0;
    let mut logged_first_audio = false;

    // VAD loop: drain shared buffer every VAD_TICK_INTERVAL.
    let result: Result<Option<()>> = loop {
        // Check stop signal before sleeping.
        if *stop.borrow() {
            // Manual stop (second Ctrl+B). Push-to-talk means "send what I just
            // said" — flush the in-progress utterance instead of discarding it,
            // unless nothing above the silence threshold was ever captured.
            let flush = flush_on_stop(
                max_rms_seen,
                vad_settings.silence_threshold,
                accumulated_16k.len(),
            );
            tracing::debug!(
                "capture_utterance: stop signal received (flush={flush}, max_rms={max_rms_seen:.0})"
            );
            break Ok(if flush { Some(()) } else { None });
        }

        tokio::time::sleep(VAD_TICK_INTERVAL).await;

        // Drain shared buffer (brief SYNC lock only).
        let chunk: Vec<i16> = match shared_buf.lock() {
            Ok(mut g) => std::mem::take(&mut *g),
            Err(_) => continue,
        };

        if chunk.is_empty() {
            continue;
        }

        total_capture_samples += chunk.len() as u64;

        // T-36.17.8-cost: hard cap on recording duration.
        if total_capture_samples >= max_capture_samples {
            tracing::warn!("capture_utterance: max_recording_seconds cap reached");
            let chunk_16k = linear_resample(&chunk, capture_rate);
            accumulated_16k.extend_from_slice(&chunk_16k);
            break Ok(Some(()));
        }

        // Resample to 16 kHz if needed (RESEARCH Pitfall 1).
        let chunk_16k = linear_resample(&chunk, capture_rate);
        accumulated_16k.extend_from_slice(&chunk_16k);

        // Diagnostics: surface audio flow + loudness vs the VAD RMS threshold (200).
        tick += 1;
        let chunk_rms = if chunk_16k.is_empty() {
            0.0
        } else {
            (chunk_16k.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / chunk_16k.len() as f64)
                .sqrt()
        };
        max_rms_seen = max_rms_seen.max(chunk_rms);
        if !logged_first_audio && chunk_rms > 0.0 {
            logged_first_audio = true;
            tracing::info!(
                "capture_utterance: first audio chunk rms={chunk_rms:.0} (silence_threshold={})",
                vad_settings.silence_threshold
            );
        }
        if tick.is_multiple_of(100) {
            tracing::debug!(
                "capture_utterance: tick={tick} total_samples={total_capture_samples} chunk_rms={chunk_rms:.0} max_rms={max_rms_seen:.0}"
            );
        }

        // Feed ONLY the newly-captured chunk to the VAD (never `accumulated_16k`).
        // `EnergyVad` is a cumulative state machine; re-feeding the whole growing
        // buffer every tick replays prior windows and inflates its time counters
        // quadratically, firing EndOfSpeech/HardStop seconds early. See `feed_vad`.
        let decision = feed_vad(&mut vad, &chunk_16k);

        match decision {
            VadDecision::Continue => {}
            VadDecision::EndOfSpeech => {
                tracing::debug!("capture_utterance: VAD EndOfSpeech");
                break Ok(Some(()));
            }
            VadDecision::HardStop => {
                tracing::debug!("capture_utterance: VAD HardStop — no speech");
                break Ok(None);
            }
        }
    };

    // Drop stream to stop capture (keep alive until here).
    drop(stream);

    // Tuning aid: report the loudest level seen vs the active threshold. If
    // `max_rms` never exceeds `silence_threshold`, no speech is ever confirmed —
    // lower `voice.silence_threshold` below this number.
    tracing::info!(
        "capture_utterance: cycle ended — max_rms={max_rms_seen:.0} silence_threshold={}",
        vad_settings.silence_threshold
    );

    match result? {
        None => Ok(None),
        Some(()) => {
            if accumulated_16k.is_empty() {
                return Ok(None);
            }
            let audio_dir = home.join("audio_cache");
            let wav_path = write_wav(&audio_dir, &accumulated_16k)?;
            tracing::info!("capture_utterance: wrote {:?}", wav_path);
            Ok(Some(wav_path))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One ~10 ms tick of audio (160 samples at 16 kHz) at a constant amplitude.
    fn tick(amplitude: i16) -> Vec<i16> {
        vec![amplitude; VAD_TICK_SAMPLES]
    }

    /// Correct strategy: feed only the new chunk each tick (what `capture_utterance`
    /// now does). Returns `(ticks, decision)` at the first non-Continue decision.
    fn ticks_to_stop_incremental(t: &[i16], max_ticks: usize) -> (usize, VadDecision) {
        let mut vad = EnergyVad::new();
        for n in 1..=max_ticks {
            let d = feed_vad(&mut vad, t);
            if d != VadDecision::Continue {
                return (n, d);
            }
        }
        (max_ticks, VadDecision::Continue)
    }

    /// The OLD bug: re-feed the entire accumulated buffer each tick.
    fn ticks_to_stop_accumulated(t: &[i16], max_ticks: usize) -> (usize, VadDecision) {
        let mut vad = EnergyVad::new();
        let mut accumulated: Vec<i16> = Vec::new();
        for n in 1..=max_ticks {
            accumulated.extend_from_slice(t);
            let mut d = VadDecision::Continue;
            for window in accumulated.chunks(VAD_TICK_SAMPLES) {
                d = vad.push_chunk(window);
                if d != VadDecision::Continue {
                    break;
                }
            }
            if d != VadDecision::Continue {
                return (n, d);
            }
        }
        (max_ticks, VadDecision::Continue)
    }

    // UAT regression: pressing Ctrl+B to stop must SEND the utterance just spoken,
    // not discard it. Before this fix, the stop signal made capture_utterance return
    // Ok(None), so a 12 s spoken utterance (max_rms 1013, threshold 60) was thrown
    // away and never transcribed. flush_on_stop now flushes captured speech and only
    // discards true silence / pre-speech stops.
    #[test]
    fn manual_stop_flushes_captured_speech_but_discards_silence() {
        let threshold = 60;
        // Spoke for a while (loud audio captured) then hit Ctrl+B → flush & transcribe.
        assert!(
            flush_on_stop(1013.0, threshold, 192_000),
            "a real spoken utterance must be flushed on manual stop, not discarded"
        );
        // Stopped immediately, nothing captured → discard (no empty WAV to STT).
        assert!(
            !flush_on_stop(0.0, threshold, 0),
            "stopping before any audio must discard"
        );
        // Only background noise below threshold → discard (no real utterance).
        assert!(
            !flush_on_stop(40.0, threshold, 96_000),
            "sub-threshold noise must not be sent as a transcript"
        );
        // Edge: loudest chunk exactly at the threshold counts as speech.
        assert!(
            flush_on_stop(60.0, threshold, 16_000),
            "rms exactly at threshold counts as captured speech"
        );
    }

    // Regression for the Ctrl+B "stops after ~3 s" bug: streaming pure silence in
    // 10 ms ticks must HardStop only near the real 15 s threshold (~1500 ticks).
    // The accumulated re-feed fired in well under 100 ticks (~0.5 s/cycle), so the
    // CLI loop burned its 3 no-speech cycles in ~2-3 s before the user could speak.
    #[test]
    fn incremental_feed_is_realtime_not_quadratic() {
        let silence = tick(0);

        let (inc_ticks, inc_dec) = ticks_to_stop_incremental(&silence, 3000);
        assert_eq!(inc_dec, VadDecision::HardStop);
        assert!(
            (1450..=1550).contains(&inc_ticks),
            "incremental feed should HardStop at ~1500 ticks (15 s); got {inc_ticks}"
        );

        let (acc_ticks, _) = ticks_to_stop_accumulated(&silence, 3000);
        assert!(
            acc_ticks < 100,
            "accumulated re-feed (the bug) should fire <100 ticks; got {acc_ticks}"
        );
        assert!(
            acc_ticks * 5 < inc_ticks,
            "incremental must be far slower (real-time) than the buggy re-feed: \
             incremental={inc_ticks} ticks vs accumulated={acc_ticks} ticks"
        );
    }

    // End-of-speech must fire only after confirmed speech THEN ~3 s of silence,
    // measured in real time — not prematurely.
    #[test]
    fn incremental_feed_end_of_speech_realtime() {
        let speech = tick(500); // RMS 500 > 200 threshold
        let silence = tick(0);
        let mut vad = EnergyVad::new();

        // 0.6 s of speech (60 ticks) → confirms speech, stays Continue.
        for _ in 0..60 {
            assert_eq!(feed_vad(&mut vad, &speech), VadDecision::Continue);
        }

        // Then silence until EndOfSpeech — expected ~3.0 s = ~300 ticks.
        let mut ticks = 0usize;
        let decision = loop {
            ticks += 1;
            let d = feed_vad(&mut vad, &silence);
            if d != VadDecision::Continue || ticks > 1000 {
                break d;
            }
        };
        assert_eq!(decision, VadDecision::EndOfSpeech);
        assert!(
            (290..=360).contains(&ticks),
            "EndOfSpeech should fire ~300 ticks (3 s) after speech; got {ticks}"
        );
    }
}
