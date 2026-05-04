//! `SubagentRunner` implementation — bridges ironhermes-agent to the
//! `SubagentRunner` trait defined in ironhermes-tools/delegate_task.rs.
//!
//! This is the agent-side half of the dependency-inversion pattern
//! (same approach as RegistryDispatch for ToolDispatch in execute_code).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use ironhermes_core::{ChatMessage, ProviderResolver};
use ironhermes_tools::ToolRegistry;
use ironhermes_tools::delegate_task::SubagentRunner;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::agent_loop::AgentLoop;
use crate::any_client::AnyClient;
use crate::budget::BudgetHandle;
use crate::subagent_registry::{SubagentInfo, SubagentRegistry};
use crate::transcript::{TranscriptLine, TranscriptWriter, transcript_path_for};

/// Concrete `SubagentRunner` that spawns child `AgentLoop` instances.
///
/// Holds a cloneable `AnyClient` so each child agent gets its own loop
/// without sharing mutable state with the parent. Supports model override
/// (D-23/D-24) via the ProviderResolver.
pub struct AgentSubagentRunner {
    /// Parent's client, used when no model override is specified.
    client: AnyClient,
    /// Provider resolver for constructing override clients.
    resolver: ProviderResolver,
    /// Optional shared iteration budget handle (PROV-10 / D-15).
    /// Plan 21.7-05 switched this from `Arc<AtomicUsize>` to [`BudgetHandle`];
    /// clones of the handle share the underlying counter so parent + child
    /// subagent loops decrement the SAME budget and observe the same
    /// pressure-tier ladder.
    budget: Option<BudgetHandle>,
    /// Plan 21.7-07 (D-03 / D-04): shared SubagentRegistry. Each `run_child`
    /// call registers its subagent on entry and unregisters on exit so the
    /// `agents: N/M` pill and `/agents list` reflect live state.
    subagent_registry: Option<Arc<RwLock<SubagentRegistry>>>,
    /// Plan 21.7-07 (D-05): HERMES_HOME root used to compose transcript
    /// paths. Together with `session_id` forms `$HERMES_HOME/subagent-transcripts/<session>/<sub>.jsonl`.
    hermes_home: Option<PathBuf>,
    /// Plan 21.7-07 (D-05): session id used in the transcript path.
    session_id: Option<String>,
}

impl AgentSubagentRunner {
    pub fn new(
        client: AnyClient,
        resolver: ProviderResolver,
        budget: Option<BudgetHandle>,
    ) -> Self {
        Self {
            client,
            resolver,
            budget,
            subagent_registry: None,
            hermes_home: None,
            session_id: None,
        }
    }

    /// Plan 21.7-07 (D-03 / D-04): attach a shared `SubagentRegistry`.
    /// Every `run_child` call registers itself on entry, appends transcript
    /// events, and unregisters on exit (including cancel).
    pub fn with_subagent_registry(mut self, reg: Arc<RwLock<SubagentRegistry>>) -> Self {
        self.subagent_registry = Some(reg);
        self
    }

    /// Plan 21.7-07 (D-05): set the transcript path scope. Both arguments
    /// must be set for transcripts to be written; either one missing leaves
    /// transcript writing disabled (registry wiring still fires).
    pub fn with_transcript_scope(mut self, hermes_home: PathBuf, session_id: String) -> Self {
        self.hermes_home = Some(hermes_home);
        self.session_id = Some(session_id);
        self
    }
}

/// Plan 21.7-07: generate a `sub_<12 hex>` subagent id.
fn random_subagent_id() -> String {
    let uuid = uuid::Uuid::new_v4();
    let bytes = uuid.as_bytes();
    // 12 hex chars = first 6 bytes.
    format!(
        "sub_{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]
    )
}

/// Plan 21.7-07: clip a goal string to the first 80 chars on a UTF-8 char
/// boundary for `SubagentInfo.task_summary`.
fn truncate_summary(goal: &str, max_chars: usize) -> String {
    if goal.chars().count() <= max_chars {
        return goal.to_string();
    }
    let mut end = 0;
    for (i, _) in goal.char_indices().take(max_chars) {
        end = i;
    }
    // advance past the last char
    let last_char_len = goal[end..]
        .chars()
        .next()
        .map(|c| c.len_utf8())
        .unwrap_or(0);
    goal[..end + last_char_len].to_string()
}

#[async_trait]
impl SubagentRunner for AgentSubagentRunner {
    async fn run_child(
        &self,
        registry: Arc<RwLock<ToolRegistry>>,
        system_prompt: String,
        max_iterations: usize,
        model_override: Option<&str>,
        cancel_token: Option<CancellationToken>,
        tool_progress: Option<ironhermes_tools::delegate_task::ChildToolProgressCallback>,
    ) -> anyhow::Result<Option<String>> {
        // D-23/D-24: construct child client with model override if specified
        let child_client = if let Some(model) = model_override {
            let endpoint = self.resolver.resolve_for_main();
            AnyClient::from_endpoint_with_model(endpoint, model)?
        } else {
            self.client.clone()
        };

        // Plan 21.7-07 (D-03 / D-04 / D-05 / D-07):
        // 1. Generate subagent_id + transcript path.
        // 2. Register SubagentInfo in the SubagentRegistry on entry.
        // 3. Keep a cancel_for_info clone so SubagentRegistry::kill can fire it.
        // 4. On exit: append TranscriptLine::{Done | Cancelled} and unregister.
        let subagent_id = random_subagent_id();
        let task_summary = derive_task_summary(&system_prompt);

        // A cancel token is REQUIRED to register; if caller didn't pass one
        // we synthesize an orphan token so SubagentInfo has a valid handle.
        // The orphan token is only used for the registry's kill path; the
        // AgentLoop still gets the original (possibly None) token via the
        // existing D-21 wiring.
        let cancel_for_info = cancel_token
            .as_ref()
            .map(|t| t.clone())
            .unwrap_or_else(CancellationToken::new);

        // Attempt to build a transcript writer; skip silently if scope
        // wasn't provided (keeps legacy tests that don't set it green).
        let transcript_writer: Option<TranscriptWriter> =
            match (self.hermes_home.as_ref(), self.session_id.as_ref()) {
                (Some(home), Some(sid)) => {
                    let path = transcript_path_for(home, sid, &subagent_id);
                    let writer = TranscriptWriter::open(path);
                    // Phase 22.3 D-07 / UI-SPEC ALIAS-1: touch the JSONL file BEFORE
                    // SubagentRegistry::register(info) below so `/agents logs <alias>`
                    // can stat the file the moment the alias appears in `/agents list`.
                    // (RESEARCH correction: ordering must be open → touch → register,
                    // NOT open → register → touch as CONTEXT D-07 originally suggested.)
                    writer.touch().await;
                    Some(writer)
                }
                _ => None,
            };

        // Register in the SubagentRegistry, if attached. The D-03/D-04 pill
        // refresh in main.rs reads `active_count()` after this registration
        // lands. Errors here are NOT possible — `register` is infallible.
        if let Some(ref reg) = self.subagent_registry {
            // Compose path for SubagentInfo even if we didn't open a writer.
            let path_for_info = transcript_writer
                .as_ref()
                .map(|w| w.path().to_path_buf())
                .unwrap_or_else(|| PathBuf::from("/dev/null"));
            let info = SubagentInfo {
                id: subagent_id.clone(),
                task_summary: task_summary.clone(),
                parent_id: None, // v2.1+ will thread parent id
                started_at: Instant::now(),
                cancel: cancel_for_info,
                transcript_path: path_for_info,
            };
            reg.write().await.register(info);
        }

        // Wrap the caller's tool_progress callback so every tool event also
        // appends a TranscriptLine (D-05: per-turn writes).
        let tool_progress_wrapped: Option<
            ironhermes_tools::delegate_task::ChildToolProgressCallback,
        > = if let Some(writer) = transcript_writer.clone() {
            let inner = tool_progress;
            Some(Box::new(move |name: &str, args_preview: &str| {
                // fire-and-forget; never stall the agent turn
                writer.append(TranscriptLine::now_tool_call(name, args_preview));
                if let Some(ref cb) = inner {
                    cb(name, args_preview);
                }
            })
                as ironhermes_tools::delegate_task::ChildToolProgressCallback)
        } else {
            tool_progress
        };

        let mut agent = AgentLoop::new(child_client, registry, max_iterations);
        // D-21: Forward cancel token to child AgentLoop
        if let Some(ref token) = cancel_token {
            agent = agent.with_cancellation_token(token.clone());
        }
        // D-19: Forward (possibly wrapped) tool progress callback
        if let Some(cb) = tool_progress_wrapped {
            agent = agent.with_tool_progress(cb);
        }
        // Wire budget if available
        if let Some(ref budget) = self.budget {
            agent = agent.with_budget(budget.clone());
        }

        let messages = vec![
            ChatMessage::system(&system_prompt),
            ChatMessage::user("Complete the task described in the system prompt."),
        ];

        let run_result = agent.run(messages).await;

        // Classify the exit path: error, cancelled, or completed.
        // A cancel_token fire BEFORE run() returns cleanly still counts as
        // cancelled for transcript audit purposes (D-07 / E-08 contract).
        let cancelled_flag = cancel_token
            .as_ref()
            .map(|t| t.is_cancelled())
            .unwrap_or(false);

        // Emit terminal transcript line (D-05 normal / D-07 cancel) + unregister.
        let (final_response, outcome) = match run_result {
            Ok(result) => (result.final_response, Ok::<(), anyhow::Error>(())),
            Err(e) => (None, Err(e)),
        };

        if let Some(writer) = transcript_writer.as_ref() {
            if cancelled_flag {
                // D-07 audit: Cancelled is the LAST line on the cancel path.
                writer.append(TranscriptLine::now_cancelled("cancelled via parent token"));
            } else if outcome.is_ok() {
                let preview = final_response.as_deref().unwrap_or("").to_string();
                writer.append(TranscriptLine::now_done(preview));
            } else {
                // Error path: still write a Done with an error preview so
                // transcripts always terminate with either Done or Cancelled.
                writer.append(TranscriptLine::now_done("(error: run_child returned Err)"));
            }
        }

        // Unregister AFTER the terminal line lands so the pill drops to N-1
        // only once the transcript file has seen its marker.
        if let Some(ref reg) = self.subagent_registry {
            reg.write().await.unregister(&subagent_id);
        }

        match outcome {
            Ok(()) => Ok(final_response),
            Err(e) => Err(e),
        }
    }
}

/// Plan 21.7-07: extract a short task summary from the system_prompt.
/// DelegateTaskTool formats its system_prompt as:
///   "You are a focused assistant. Complete the following task:\n\n{goal}"
/// We strip the prefix and take the first 80 chars of the goal.
fn derive_task_summary(system_prompt: &str) -> String {
    const PREFIX: &str = "You are a focused assistant. Complete the following task:\n\n";
    let tail = system_prompt.strip_prefix(PREFIX).unwrap_or(system_prompt);
    // Take up through the first blank line (goal is separated from context by \n\n)
    let goal = tail.split("\n\n").next().unwrap_or(tail);
    truncate_summary(goal, 80)
}

#[cfg(test)]
mod plan_21_7_07_helpers_tests {
    use super::*;

    #[test]
    fn random_subagent_id_has_sub_prefix_and_12_hex() {
        let id = random_subagent_id();
        assert!(
            id.starts_with("sub_"),
            "id must start with 'sub_'; got {}",
            id
        );
        let hex = &id[4..];
        assert_eq!(hex.len(), 12, "hex portion must be 12 chars; got {}", hex);
        assert!(
            hex.chars().all(|c| c.is_ascii_hexdigit()),
            "hex portion must be [0-9a-f]; got {}",
            hex
        );
    }

    #[test]
    fn random_subagent_id_unique_across_calls() {
        let a = random_subagent_id();
        let b = random_subagent_id();
        assert_ne!(a, b, "consecutive ids must differ");
    }

    #[test]
    fn truncate_summary_respects_char_boundary() {
        // 80 ASCII chars → unchanged
        let short = "a".repeat(80);
        assert_eq!(truncate_summary(&short, 80).len(), 80);
        // 85 ASCII chars → 80
        let longer = "a".repeat(85);
        assert_eq!(truncate_summary(&longer, 80).chars().count(), 80);
        // Mixed multibyte: 10 emoji → 4 emoji
        let emoji: String = "🌟".repeat(10);
        let out = truncate_summary(&emoji, 4);
        assert_eq!(out.chars().count(), 4);
    }

    #[test]
    fn derive_task_summary_strips_delegate_task_prefix() {
        let prompt = "You are a focused assistant. Complete the following task:\n\nAdd a test for foo\n\nContext:\nmore";
        assert_eq!(derive_task_summary(prompt), "Add a test for foo");
    }

    #[test]
    fn derive_task_summary_falls_back_when_prefix_absent() {
        let prompt = "random prompt";
        assert_eq!(derive_task_summary(prompt), "random prompt");
    }
}
