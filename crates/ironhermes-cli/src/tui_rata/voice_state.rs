// Phase 36.17.8 — TUI voice state (D-08 Ctrl+B push-to-talk + continuous VAD loop).
//
// `VoiceState` mirrors the `queue`/`queue_paused` Arc-field pattern (PATTERNS §2):
// all toggle fields are Arc<AtomicBool> so slash-command closures can mutate without
// holding `&mut App`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::task::JoinHandle;

/// Phase recorded by the voice loop during an active capture cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordPhase {
    /// Not recording.
    #[default]
    Idle,
    /// cpal stream open, EnergyVad running.
    Listening,
    /// WAV captured, STT request in flight.
    Transcribing,
}

/// TUI voice state — held as a field on `App`.
///
/// All flags are Arc<AtomicBool> per PATTERNS §2: slash handlers and the
/// capture-task closure both reference these without holding `&mut App`.
pub struct VoiceState {
    /// True when voice mode is active (`/voice on` was issued or Ctrl+B is in use).
    pub enabled: Arc<AtomicBool>,

    /// True when the capture loop is currently recording (Ctrl+B pressed, not yet done).
    pub recording: Arc<AtomicBool>,

    /// Opt-in spoken reply toggle (`/voice tts`). When true, the capture loop
    /// calls the shipped `send_audio` path after each agent response (D-11/D-16).
    /// Defaults false — text replies are the default.
    pub auto_tts: Arc<AtomicBool>,

    /// Current phase within the capture/transcribe cycle (for status pill).
    pub phase: Arc<std::sync::Mutex<RecordPhase>>,

    /// Handle to the running `run_conversation_loop` tokio task.
    /// Dropped (which cancels the task via the stop watch-sender) when the user
    /// presses Ctrl+B a second time or issues `/voice off`.
    pub task_handle: Option<JoinHandle<()>>,

    /// Sender half of the stop watch-channel. Setting to `true` cancels capture.
    pub stop_tx: Option<tokio::sync::watch::Sender<bool>>,
}

impl VoiceState {
    /// Create a new idle VoiceState.
    pub fn new() -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(false)),
            recording: Arc::new(AtomicBool::new(false)),
            auto_tts: Arc::new(AtomicBool::new(false)),
            phase: Arc::new(std::sync::Mutex::new(RecordPhase::Idle)),
            task_handle: None,
            stop_tx: None,
        }
    }

    /// Returns true when voice mode is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Returns true when the capture loop is actively recording.
    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }

    /// Read the current recording phase (for status pill).
    pub fn current_phase(&self) -> RecordPhase {
        self.phase.lock().map(|g| *g).unwrap_or(RecordPhase::Idle)
    }

    /// Set the recording phase (called from the capture task).
    pub fn set_phase(&self, p: RecordPhase) {
        if let Ok(mut g) = self.phase.lock() {
            *g = p;
        }
    }

    /// Stop any active capture task and reset flags to Idle.
    pub fn stop(&mut self) {
        // Signal the capture task to stop.
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(true);
        }
        // Drop the task handle (task will receive stop signal and exit).
        self.task_handle = None;
        self.recording.store(false, Ordering::Relaxed);
        self.set_phase(RecordPhase::Idle);
    }

    /// Toggle auto_tts and return the new state.
    pub fn toggle_auto_tts(&self) -> bool {
        let was = self.auto_tts.load(Ordering::Relaxed);
        self.auto_tts.store(!was, Ordering::Relaxed);
        !was
    }
}

impl Default for VoiceState {
    fn default() -> Self {
        Self::new()
    }
}
