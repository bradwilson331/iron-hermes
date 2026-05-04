//! Per-turn streaming event enum for the tui_rata REPL (Phase 22.4 D-17).
//!
//! Carries deltas, tool lifecycle, and turn termination signals across a
//! `tokio::sync::mpsc::UnboundedChannel` bridge between a spawned
//! `AgentLoop::run_agent_turn` task and the main event loop. The per-turn
//! `UnboundedSender<StreamEvent>` is dropped when the turn completes —
//! channel close signals "no more deltas this turn."
//!
//! # D-17 canonical variants
//!
//! The 8 variants below are locked by CONTEXT D-17. Additional variants
//! (e.g. `MemoryWrite`, `HookFired`, `McpReconnect`) may be added by a
//! future phase with planner justification, but this phase stays at 8
//! per RESEARCH Open Question §3 — no speculative additions.

#[derive(Debug)]
pub enum StreamEvent {
    /// Fired once at turn start (before any `Delta`).
    Started,
    /// Per-token delta from the LLM stream. Concatenate into
    /// `App::assistant_buffer`.
    Delta(String),
    /// The model invoked a tool. Update status pill `hint` field.
    ToolCall { name: String },
    /// Progress update from a long-running tool. Optional per-tool.
    ToolProgress { name: String, phase: String },
    /// Tool invocation finished. `ok=false` surfaces as an error-coloured
    /// status hint.
    ToolResult { name: String, ok: bool },
    /// Turn completed cleanly (all deltas + tool calls applied).
    Finished,
    /// Turn ended with an error. String is already user-facing (no PII
    /// stripping is this enum's responsibility).
    Error(String),
    /// Turn was cancelled by user (Ctrl+C → D-12 state machine → token cancel).
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_canonical_variants_compile_and_debug() {
        // Compile-time + Debug coverage of all 8 canonical variants.
        let events = vec![
            StreamEvent::Started,
            StreamEvent::Delta("hello".to_string()),
            StreamEvent::ToolCall {
                name: "bash".to_string(),
            },
            StreamEvent::ToolProgress {
                name: "bash".to_string(),
                phase: "running".to_string(),
            },
            StreamEvent::ToolResult {
                name: "bash".to_string(),
                ok: true,
            },
            StreamEvent::Finished,
            StreamEvent::Error("timeout".to_string()),
            StreamEvent::Cancelled,
        ];
        assert_eq!(events.len(), 8, "D-17 locks exactly 8 canonical variants");
        for event in &events {
            let _ = format!("{event:?}");
        }
    }
}
