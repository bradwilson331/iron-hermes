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
use crate::any_client::{AnyClient, wire_fallback_if_configured};
use crate::budget::BudgetHandle;
use crate::subagent_registry::{RegistrationGuard, SubagentInfo, SubagentRegistry};
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
    /// Phase 32.2 D-04: current depth in the spawn tree.
    /// 0 = root caller (the runner attached to the top-level DelegateTaskTool);
    /// children spawned by an orchestrator child at depth N run at depth N+1.
    /// Depth threading is internal to AgentSubagentRunner state — the
    /// SubagentRunner trait signature is NOT changed (per RESEARCH Pitfall 6).
    pub current_depth: u32,
    /// Phase 32.2 D-10: the subagent_id of the runner's parent caller.
    /// None = root (this runner is attached to the top-level parent agent).
    /// Populated via `with_caller_id`; Plan 04 wires it into SubagentInfo.parent_id.
    pub caller_subagent_id: Option<String>,
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
            current_depth: 0,
            caller_subagent_id: None,
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

    /// Phase 32.2 D-04: set the current depth in the spawn tree.
    /// Root caller is depth 0; children of an orchestrator at depth N run at depth N+1.
    /// Use this builder when constructing a runner for an orchestrator child's DelegateTaskTool
    /// so that `build_child_registry` receives the correct depth for gating further nesting.
    pub fn with_current_depth(mut self, depth: u32) -> Self {
        self.current_depth = depth;
        self
    }

    /// Phase 32.2 D-10: set the caller's subagent_id for parent-child tracing.
    /// Plan 04 wires this into `SubagentInfo.parent_id` for the `/agents` tree view.
    pub fn with_caller_id(mut self, id: String) -> Self {
        self.caller_subagent_id = Some(id);
        self
    }

    /// Test-only constructor that creates a runner with dummy client state.
    /// Used to test field initialization and builder methods without network calls.
    #[cfg(test)]
    fn new_for_test() -> Self {
        use crate::client::LlmClient;
        let dummy_client = AnyClient::ChatCompletions(LlmClient::new(
            "http://localhost:9999",
            "dummy-key",
            "dummy-model",
        ));
        // SAFETY: ProviderResolver is only used in run_child, which is not called
        // in field-inspection tests. We build a real resolver from a minimal Config.
        let config = ironhermes_core::Config::default();
        let resolver = ironhermes_core::ProviderResolver::build(&config)
            .expect("default Config should produce a valid resolver");
        Self::new(dummy_client, resolver, None)
    }
}

/// Phase 32.2 Plan 04 (D-10): Build a `SubagentInfo` from the components
/// available at registration time inside `run_child`.
///
/// Extracted as a pure helper so the parent_id wiring can be unit-tested
/// without driving the full async `run_child` path (which requires live
/// clients, AgentLoop, etc.). Production code calls this; tests call it directly.
pub(crate) fn build_subagent_info(
    subagent_id: String,
    task_summary: String,
    caller_subagent_id: Option<String>,
    cancel: tokio_util::sync::CancellationToken,
    transcript_path: std::path::PathBuf,
) -> SubagentInfo {
    SubagentInfo {
        id: subagent_id,
        task_summary,
        parent_id: caller_subagent_id,
        started_at: std::time::Instant::now(),
        cancel,
        transcript_path,
        // Phase 32.3 Plan 01 (D-04 reservation): live activity clock
        // reservation — Plan 02 wires the real Arc<Mutex<Instant>> from the
        // child AgentLoop. Initialise None here.
        activity_last: None,
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

        // Phase 32.3 Plan 01 (D-01 / D-02 / D-03): register via the RAII guard.
        // `_guard` is bound for the entire remaining body of `run_child`; its
        // Drop calls `unregister_internal` synchronously on every exit path
        // (natural return, error, tokio::time::timeout future-drop, panic,
        // cancel). The D-03/D-04 pill refresh in main.rs reads `active_count()`
        // after this registration lands. `register_guarded` is infallible.
        let _guard: Option<RegistrationGuard> = if let Some(ref reg) = self.subagent_registry {
            // Compose path for SubagentInfo even if we didn't open a writer.
            let path_for_info = transcript_writer
                .as_ref()
                .map(|w| w.path().to_path_buf())
                .unwrap_or_else(|| PathBuf::from("/dev/null"));
            let info = SubagentInfo {
                id: subagent_id.clone(),
                task_summary: task_summary.clone(),
                parent_id: self.caller_subagent_id.clone(), // Phase 32.2 Plan 04 D-10: populated
                started_at: Instant::now(),
                cancel: cancel_for_info,
                transcript_path: path_for_info,
                // Phase 32.3 Plan 01 reservation; wired by Plan 02.
                activity_last: None,
            };
            let weak = Arc::downgrade(reg);
            Some(reg.write().await.register_guarded(info, weak))
        } else {
            None
        };

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
        // Wire fallback so subagent retries on primary model failure (PROV-07 / phase 27.1.4.1)
        agent = wire_fallback_if_configured(agent, &self.resolver); // chains .with_fallback() via the shared helper — PROV-07
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

        // Phase 32.3 Plan 01: RegistrationGuard drop handles deregistration on
        // all exit paths via `_guard` going out of scope. Keep the explicit
        // unregister_internal call below — Plan 02 deletes it. With the guard
        // in place this call is redundant but safe: `unregister_internal` on
        // an absent id is a no-op. The 6.7-hour ghost bug (sub_20667cb71808)
        // was that `tokio::time::timeout` dropped this function's future
        // before reaching this line — the guard's Drop is what closes that
        // gap structurally.
        if let Some(ref reg) = self.subagent_registry {
            reg.write().await.unregister_internal(&subagent_id);
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
mod tests {
    use super::*;

    /// Build a minimal `AgentSubagentRunner` for field-inspection tests.
    fn make_runner() -> AgentSubagentRunner {
        AgentSubagentRunner::new_for_test()
    }

    #[test]
    fn test_default_current_depth_is_zero() {
        // Phase 32.2 D-04: new() must initialise current_depth=0 (root caller)
        // and caller_subagent_id=None (no parent).
        let runner = make_runner();
        assert_eq!(
            runner.current_depth, 0,
            "default current_depth must be 0 (root caller)"
        );
        assert!(
            runner.caller_subagent_id.is_none(),
            "default caller_subagent_id must be None (root)"
        );
    }

    #[test]
    fn test_with_current_depth_sets_field() {
        // Phase 32.2 D-04: with_current_depth(N) must set current_depth to N.
        let runner = make_runner().with_current_depth(2);
        assert_eq!(
            runner.current_depth, 2,
            "with_current_depth(2) must set field to 2"
        );
    }

    #[test]
    fn test_with_caller_id_sets_field() {
        // Phase 32.2 D-10: with_caller_id(id) must set caller_subagent_id to Some(id).
        let runner = make_runner().with_caller_id("sub_abc123".to_string());
        assert_eq!(
            runner.caller_subagent_id,
            Some("sub_abc123".to_string()),
            "with_caller_id must set caller_subagent_id to Some(id)"
        );
    }

    #[test]
    fn test_run_child_propagates_caller_id_to_subagent_info() {
        // Phase 32.2 Plan 04 D-10: verify that build_subagent_info wires
        // caller_subagent_id → SubagentInfo.parent_id correctly.
        //
        // We test the extracted pure helper directly rather than driving the
        // full async run_child path (which requires a live client + AgentLoop).
        // This matches the plan's "extract a fn build_subagent_info(...) pure
        // helper and test it directly" guidance.
        let caller_id = "parent_sub_abc".to_string();
        let cancel = tokio_util::sync::CancellationToken::new();
        let info = build_subagent_info(
            "sub_child001".to_string(),
            "do the child task".to_string(),
            Some(caller_id.clone()),
            cancel,
            std::path::PathBuf::from("/dev/null"),
        );

        assert_eq!(
            info.parent_id,
            Some(caller_id),
            "build_subagent_info must wire caller_subagent_id into SubagentInfo.parent_id"
        );
        assert_eq!(info.id, "sub_child001");
        assert_eq!(info.task_summary, "do the child task");

        // Also verify None propagates correctly (root runner case)
        let cancel2 = tokio_util::sync::CancellationToken::new();
        let root_info = build_subagent_info(
            "sub_root".to_string(),
            "root task".to_string(),
            None,
            cancel2,
            std::path::PathBuf::from("/dev/null"),
        );
        assert!(
            root_info.parent_id.is_none(),
            "root runner (caller_subagent_id=None) must produce SubagentInfo.parent_id=None"
        );
    }
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
