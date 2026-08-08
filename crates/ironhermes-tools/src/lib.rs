pub mod approval;
pub mod artifact; // Phase 46.6 D-01/D-03/D-05/D-06 — artifact tool
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
pub mod capture; // Phase 36.17.8 — mic capture (cpal-backed)
pub mod chat_capture; // Phase 46.7 D-13/D-14/D-15/D-16/D-23/D-25 — chat-turn deliverable capture + path containment
pub mod clarify_registry; // Phase 36.3.8 Plan 02 — PendingClarifyRegistry + ClarifyAnswer
pub mod clarify_tool; // Phase 36.3.8 Plan 02 — ClarifyTool + ClarifyDispatcher trait
pub mod credentials; // Phase 41.3 D-18/D-19 — ToolCredentials env→config→vault snapshot
pub mod cronjob_tool;
pub mod delegate_task;
pub mod execute_code;
pub mod fal; // Phase 01 — fal.ai queue REST client (FalClient)
pub mod file_tools;
pub mod gen_backend; // Phase 47 Plan 04 — provider dispatch seam (GenBackend::resolve)
pub mod gen_guardrail; // Phase 47 Plan 05 — shared spend-guardrail primitive (GenerationGuardrail)
pub mod hallucination_filter; // Phase 36.17.8 — STT hallucination filter
pub mod hexapod_tcp; // Phase 27.1.1 — registration in Plan 04 register_defaults
pub mod hexapod_video; // Phase 27.1.4 — stateless single-frame JPEG capture via port 8002
pub mod image_gen; // Phase 01 — fal.ai text-to-image LLM tool (ImageGenTool)
pub mod memory_manager_handle;
pub mod memory_tool;
pub mod not_supported_dispatcher; // Phase 36.17.7 D-03-b — zero-impl AudioDispatcher stub for Discord/Slack
pub mod registry;
pub mod send_audio_tool; // Phase 36.17.5 D-14/D-15 — SendAudioTool + AudioDispatcher
pub mod send_message_tool; // Phase 36.3.8 D-01/D-02/D-03 — SendMessageTool + MessageDispatcher
pub mod skill_manage; // Phase 33 — learning toolset (LEARN-04, LEARN-05)
pub mod skills_tool;
pub mod stt; // Phase 36.17.8 — STT provider impls (groq, openai)
pub mod terminal;
pub mod toolset_session; // Phase 25.2 Plan 15 — production ToolsetSessionHandle impl (UAT Issue 2)
pub mod tts; // Phase 36.17.5 — TTS provider impls (edge, elevenlabs)
pub mod tts_tool; // Phase 36.17.5 D-05/D-06/D-07 — TextToSpeechTool LLM tool
pub mod vad; // Phase 36.17.8 — Voice Activity Detection
pub mod venice; // Phase 47 Plan 02 — Venice.ai HTTP client (VeniceClient)
pub mod video_gen; // Phase 36.3.3 Plan 02 — fal.ai video LLM tools (VideoGenerateTool, VideoAnimateTool)
pub mod video_to_video; // Phase 47 Plan 07 D-14 — net-new video-to-video LLM tool (VideoToVideoTool)
pub mod web_answer; // Phase 41.3 D-07/D-13 (Plan 08) — synthesized-answer half of the web_search/web_answer split
pub mod web_extract; // Phase 25.2
pub mod web_local; // Phase 25.2 — shared HTML→Markdown helpers (extract_content_local target)
pub mod web_read;
pub mod web_search;

pub use clarify_registry::{ClarifyAnswer, PendingClarify, PendingClarifyRegistry};
pub use clarify_tool::{ClarifyDispatcher, clarify_callback_data, parse_clarify_callback};
pub use memory_manager_handle::MemoryManagerHandle;
pub use not_supported_dispatcher::NotSupportedAudioDispatcher;
pub use registry::{
    InterceptHandler, Prerequisite, Tool, ToolRegistry, todo_read_schema, todo_write_schema,
};
pub use send_audio_tool::{AudioDispatcher, SendAudioTool};
pub use send_message_tool::{MessageDispatcher, SendMessageTool};
pub use toolset_session::{RegistryToolsetSession, known_tool_names, tool_names_among};
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
