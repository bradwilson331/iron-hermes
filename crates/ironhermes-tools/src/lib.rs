pub mod approval;
pub mod browser_back; // Phase 25.1 — implemented by plan 04
pub mod browser_click; // Phase 25.1 — implemented by plan 06
pub mod browser_close; // Phase 25.1 — implemented by plan 04
pub mod browser_console; // Phase 25.1 — implemented by plan 07
pub mod browser_get_images; // Phase 25.1 — implemented by plan 05
pub mod browser_navigate; // Phase 25.1 — implemented by plan 04
pub mod browser_press; // Phase 25.1 — implemented by plan 03
pub mod browser_scroll; // Phase 25.1 — implemented by plan 03
pub mod browser_session; // Phase 25.1 — implemented by plan 02
pub mod browser_snapshot; // Phase 25.1 — implemented by plan 05
pub mod browser_type; // Phase 25.1 — implemented by plan 06
pub mod browser_vision; // Phase 25.1 — implemented by plan 08
pub mod cronjob_tool;
pub mod delegate_task;
pub mod execute_code;
pub mod file_tools;
pub mod hexapod_tcp; // Phase 27.1.1 — registration in Plan 04 register_defaults
pub mod hexapod_video; // Phase 27.1.4 — stateless single-frame JPEG capture via port 8002
pub mod memory_manager_handle;
pub mod memory_tool;
pub mod not_supported_dispatcher; // Phase 36.17.7 D-03-b — zero-impl AudioDispatcher stub for Discord/Slack
pub mod registry;
pub mod send_audio_tool; // Phase 36.17.5 D-14/D-15 — SendAudioTool + AudioDispatcher
pub mod skill_manage; // Phase 33 — learning toolset (LEARN-04, LEARN-05)
pub mod skills_tool;
pub mod terminal;
pub mod toolset_session; // Phase 25.2 Plan 15 — production ToolsetSessionHandle impl (UAT Issue 2)
pub mod tts; // Phase 36.17.5 — TTS provider impls (edge, elevenlabs)
pub mod tts_tool; // Phase 36.17.5 D-05/D-06/D-07 — TextToSpeechTool LLM tool
pub mod web_extract; // Phase 25.2
pub mod web_local; // Phase 25.2 — shared HTML→Markdown helpers (extract_content_local target)
pub mod web_read;
pub mod web_search;

pub use memory_manager_handle::MemoryManagerHandle;
pub use not_supported_dispatcher::NotSupportedAudioDispatcher;
pub use registry::{
    InterceptHandler, Prerequisite, Tool, ToolRegistry, todo_read_schema, todo_write_schema,
};
pub use send_audio_tool::{AudioDispatcher, SendAudioTool};
pub use toolset_session::RegistryToolsetSession;
pub use tts_tool::TextToSpeechTool;
pub use web_extract::WebExtractTool;

// ---------------------------------------------------------------------------
// Crate-level test utilities
// ---------------------------------------------------------------------------

/// Shared env-var serialization lock for all hexapod test modules.
///
/// Replaces the per-module `static ENV_LOCK` in `hexapod_tcp::tests` and
/// `hexapod_video::tests`, which raced on `HEXAPOD_IP` across modules.
/// All call sites use: `ENV_LOCK.lock().await`
#[cfg(test)]
pub(crate) static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
