use anyhow::{Context, Result};
use ironhermes_core::{ChatMessage, ChatResponse, ToolCall, ToolSchema, Usage};
use ironhermes_hooks::{HookEvent, HookEventKind, HookRegistry};
use ironhermes_state::StateStore;
use ironhermes_tools::ToolRegistry;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::any_client::AnyClient;
use crate::client::{StreamEvent, ToolCallDelta};
use crate::context_compressor::ContextCompressor;
use crate::subdir_discovery::SubdirDiscovery;

/// Result of an agent loop execution.
#[derive(Debug)]
pub struct AgentResult {
    /// Full conversation history including tool calls.
    pub messages: Vec<ChatMessage>,
    /// Number of LLM turns used.
    pub turns_used: usize,
    /// Whether the agent finished naturally (not by hitting max iterations).
    pub finished_naturally: bool,
    /// Final text response from the agent.
    pub final_response: Option<String>,
    /// Aggregated token usage.
    pub total_usage: AggregatedUsage,
}

#[derive(Debug, Default)]
pub struct AggregatedUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}

impl AggregatedUsage {
    fn add(&mut self, usage: &Usage) {
        self.prompt_tokens += usage.prompt_tokens;
        self.completion_tokens += usage.completion_tokens;
        self.total_tokens += usage.total_tokens;
    }
}

/// Callback for streaming content to the user.
pub type StreamCallback = Box<dyn Fn(&str) + Send + Sync>;

/// Callback for tool execution progress.
pub type ToolProgressCallback = Box<dyn Fn(&str, &str) + Send + Sync>;

/// The main agent loop that orchestrates LLM calls and tool execution.
pub struct AgentLoop {
    client: AnyClient,
    registry: Arc<ToolRegistry>,
    max_iterations: usize,
    compressor: Option<Mutex<ContextCompressor>>,
    stream_callback: Option<StreamCallback>,
    tool_progress_callback: Option<ToolProgressCallback>,
    streaming: bool,
    hook_registry: Option<Arc<HookRegistry>>,
    request_id: String,
    active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
    /// Optional cancellation token for cooperative shutdown (D-21).
    /// When cancelled, the loop returns early with "Cancelled by parent".
    cancel_token: Option<CancellationToken>,
    /// Shared iteration budget counter (PROV-09, PROV-10).
    /// Tracks total turns across parent + child agents.
    /// None = use local turns_used only (backward compat).
    budget: Option<Arc<AtomicUsize>>,
    /// Fallback client to swap to on qualifying errors (PROV-07).
    /// Set via with_fallback(). None = no fallback available.
    fallback_client: Option<AnyClient>,
    /// Whether fallback has already been activated (one-shot per D-11).
    fallback_activated: bool,
    /// Progressive subdirectory context discovery (CTX-03/CTX-04).
    /// When set, file-access tools trigger context file discovery.
    subdir_discovery: Option<Arc<std::sync::Mutex<SubdirDiscovery>>>,
    /// Optional StateStore for session_search tool interception (D-07).
    /// When set, session_search calls are handled directly without registry dispatch.
    state_store: Option<Arc<std::sync::Mutex<StateStore>>>,
}

impl AgentLoop {
    pub fn new(client: AnyClient, registry: Arc<ToolRegistry>, max_iterations: usize) -> Self {
        Self {
            client,
            registry,
            max_iterations,
            compressor: None,
            stream_callback: None,
            tool_progress_callback: None,
            streaming: false,
            hook_registry: None,
            request_id: uuid::Uuid::new_v4().to_string(),
            active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
            cancel_token: None,
            budget: None,
            fallback_client: None,
            fallback_activated: false,
            subdir_discovery: None,
            state_store: None,
        }
    }

    /// Set a cancellation token for cooperative shutdown (D-21).
    /// When the token is cancelled, the agent loop returns early.
    pub fn with_cancellation_token(mut self, token: CancellationToken) -> Self {
        self.cancel_token = Some(token);
        self
    }

    pub fn with_active_skills(mut self, active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>) -> Self {
        self.active_skills = active_skills;
        self
    }

    pub fn active_skills(&self) -> Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>> {
        self.active_skills.clone()
    }

    /// Set subdirectory discovery for progressive context injection (CTX-03/CTX-04).
    pub fn with_subdir_discovery(mut self, discovery: Arc<std::sync::Mutex<SubdirDiscovery>>) -> Self {
        self.subdir_discovery = Some(discovery);
        self
    }

    /// Set the StateStore for session_search tool interception (D-07).
    /// When set, session_search calls are intercepted before registry dispatch.
    pub fn with_state_store(mut self, store: Arc<std::sync::Mutex<StateStore>>) -> Self {
        self.state_store = Some(store);
        self
    }

    pub fn with_compression(mut self, context_length: usize, threshold: f64) -> Self {
        self.compressor = Some(Mutex::new(ContextCompressor::new(
            context_length,
            threshold,
        )));
        self
    }

    pub fn with_streaming(mut self, callback: StreamCallback) -> Self {
        self.streaming = true;
        self.stream_callback = Some(callback);
        self
    }

    pub fn with_tool_progress(mut self, callback: ToolProgressCallback) -> Self {
        self.tool_progress_callback = Some(callback);
        self
    }

    pub fn with_hook_registry(mut self, registry: Arc<HookRegistry>) -> Self {
        self.hook_registry = Some(registry);
        self
    }

    /// Set a shared iteration budget counter (PROV-09, PROV-10).
    /// When set, the budget is incremented per turn and checked for hard stop at 100%.
    pub fn with_budget(mut self, budget: Arc<AtomicUsize>) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Get the budget counter for sharing with child agents (PROV-10).
    pub fn budget(&self) -> Option<Arc<AtomicUsize>> {
        self.budget.clone()
    }

    /// Set a fallback client for one-shot provider switching (PROV-07).
    /// When the primary client fails with 429/5xx/401, the fallback client
    /// is swapped in and retries reset. Only fires once per agent run.
    pub fn with_fallback(mut self, client: AnyClient) -> Self {
        self.fallback_client = Some(client);
        self
    }

    /// Check budget threshold and return system prompt injection if needed.
    /// Returns None if below 70%, Some(message) at 70%+ and 90%+.
    fn check_budget_threshold(&self) -> Option<&'static str> {
        let budget = match &self.budget {
            Some(b) => b,
            None => return None,
        };
        let used = budget.load(Ordering::SeqCst);
        let pct = if self.max_iterations > 0 {
            used * 100 / self.max_iterations
        } else {
            0
        };
        match pct {
            100.. => None, // hard stop handled separately
            90..=99 => Some("[Warning] You are at 90% of your iteration budget. Respond to the user now with a summary of your progress. Do not start new tool calls."),
            70..=89 => Some("[Caution] You are at 70% of your iteration budget. Consolidate your work and avoid unnecessary tool calls."),
            _ => None,
        }
    }

    /// Classify an error for fallback decision-making.
    /// Returns (should_retry, should_fallback).
    fn classify_llm_error(err: &anyhow::Error) -> (bool, bool) {
        let err_str = err.to_string();
        if err_str.contains("status: 429")
            || err_str.contains("status: 500")
            || err_str.contains("status: 502")
            || err_str.contains("status: 503")
            || err_str.contains("status: 504")
        {
            return (true, true);
        }
        if err_str.contains("status: 401")
            || err_str.contains("status: 403")
            || err_str.contains("status: 404")
        {
            return (false, true);
        }
        (true, false)
    }

    /// Fire a hook event fire-and-forget. No-op if no hook registry is configured.
    fn fire_hook(&self, kind: HookEventKind) {
        if let Some(ref registry) = self.hook_registry {
            let event = HookEvent::new(&self.request_id, kind);
            registry.fire(event);
        }
    }

    /// Run the agent loop with the given messages.
    ///
    /// The loop continues until:
    /// - The LLM produces a response with no tool calls (natural completion)
    /// - Max iterations are reached
    /// - An unrecoverable error occurs
    pub async fn run(&mut self, mut messages: Vec<ChatMessage>) -> Result<AgentResult> {
        let mut tool_schemas = self.registry.get_definitions(None);
        // D-07: Add session_search schema when state_store is configured.
        // Not added in subagent context (subagents should not search sessions).
        if self.state_store.is_some() {
            tool_schemas.push(crate::session_search::session_search_schema());
        }
        let tools_option = if tool_schemas.is_empty() {
            None
        } else {
            Some(tool_schemas)
        };

        let mut turns_used = 0;
        let mut total_usage = AggregatedUsage::default();
        let mut final_response = None;

        info!(max_iterations = self.max_iterations, "Starting agent loop");

        // Note: MessageReceived is NOT fired here. It is fired by the platform layer
        // (handler.rs for Telegram, runner.rs for cron) which knows the real platform
        // and chat_id. Firing it here would produce duplicate events (Issue #4 fix).

        loop {
            // D-21: Check cancellation token before each iteration
            if let Some(ref token) = self.cancel_token {
                if token.is_cancelled() {
                    info!(turns = turns_used, "Agent loop cancelled by parent");
                    return Ok(AgentResult {
                        messages,
                        turns_used,
                        finished_naturally: false,
                        final_response: Some("Cancelled by parent".to_string()),
                        total_usage,
                    });
                }
            }

            if turns_used >= self.max_iterations {
                warn!(turns = turns_used, "Max iterations reached");
                break;
            }

            // PROV-09: Check shared budget hard stop at 100%
            if let Some(ref budget) = self.budget {
                let used = budget.load(Ordering::SeqCst);
                if used >= self.max_iterations {
                    warn!(budget_used = used, max = self.max_iterations, "Shared budget exhausted");
                    break;
                }
            }

            // Check for context compression
            if let Some(ref compressor) = self.compressor {
                let mut comp = compressor.lock().await;
                comp.compress(&mut messages);
            }

            turns_used += 1;
            // Increment shared budget counter if present (PROV-09, PROV-10)
            if let Some(ref budget) = self.budget {
                budget.fetch_add(1, Ordering::SeqCst);
            }
            debug!(turn = turns_used, messages = messages.len(), "Agent loop turn");

            // PROV-09: Inject budget threshold message if needed
            if let Some(injection) = self.check_budget_threshold() {
                messages.push(ChatMessage::system(injection));
            }

            // Call LLM with retry and fallback support
            const MAX_RETRIES: usize = 3;
            let mut retry_count = 0;

            let (assistant_message, usage) = loop {
                let llm_result = if let Some(ref token) = self.cancel_token {
                    tokio::select! {
                        result = async {
                            if self.streaming {
                                self.call_llm_streaming(&messages, tools_option.as_deref()).await
                            } else {
                                self.call_llm(&messages, tools_option.as_deref()).await
                            }
                        } => result,
                        _ = token.cancelled() => {
                            info!(turns = turns_used, "Agent loop cancelled during LLM call");
                            return Ok(AgentResult {
                                messages,
                                turns_used,
                                finished_naturally: false,
                                final_response: Some("Cancelled by parent".to_string()),
                                total_usage,
                            });
                        }
                    }
                } else if self.streaming {
                    self.call_llm_streaming(&messages, tools_option.as_deref()).await
                } else {
                    self.call_llm(&messages, tools_option.as_deref()).await
                };

                match llm_result {
                    Ok(result) => break result,
                    Err(err) => {
                        let (should_retry, should_fallback) = Self::classify_llm_error(&err);

                        // Try fallback if available and not already activated (PROV-07, D-11)
                        if should_fallback && !self.fallback_activated {
                            if let Some(fallback) = self.fallback_client.take() {
                                warn!("Primary LLM failed, activating fallback provider: {err}");
                                self.client = fallback;
                                self.fallback_activated = true;
                                retry_count = 0;
                                continue;
                            }
                        }

                        // Retry transient errors with backoff
                        if should_retry && retry_count < MAX_RETRIES {
                            retry_count += 1;
                            warn!(retry = retry_count, "LLM call failed, retrying: {err}");
                            tokio::time::sleep(tokio::time::Duration::from_millis(500 * retry_count as u64)).await;
                            continue;
                        }

                        // Exhausted retries and fallback
                        return Err(err);
                    }
                }
            };

            if let Some(ref usage) = usage {
                total_usage.add(usage);
            }

            // Check for tool calls
            let has_tool_calls = assistant_message
                .tool_calls
                .as_ref()
                .is_some_and(|tc| !tc.is_empty());

            // Extract text response
            if let Some(text) = assistant_message.content_text()
                && !text.is_empty()
            {
                final_response = Some(text.to_string());
            }

            messages.push(assistant_message.clone());

            if !has_tool_calls {
                debug!(turn = turns_used, "Agent completed naturally (no tool calls)");
                break;
            }

            // Execute tool calls
            let tool_calls = assistant_message.tool_calls.as_ref().unwrap();
            debug!(count = tool_calls.len(), "Executing tool calls");

            for tool_call in tool_calls {
                let result = self.execute_tool_call(tool_call).await;
                messages.push(ChatMessage::tool_result(
                    &tool_call.id,
                    result,
                ));
            }
        }

        // Note: ResponseSent is NOT fired here. It is fired by the platform layer
        // (handler.rs for Telegram, runner.rs for cron) which knows the real platform
        // and chat_id. Firing it here would produce duplicate events (Issue #4 fix).

        Ok(AgentResult {
            messages,
            turns_used,
            finished_naturally: turns_used < self.max_iterations,
            final_response,
            total_usage,
        })
    }

    /// Call LLM without streaming.
    async fn call_llm(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolSchema]>,
    ) -> Result<(ChatMessage, Option<Usage>)> {
        let response: ChatResponse = self
            .client
            .chat_completion(messages, tools, None, None, None, None)
            .await
            .context("LLM call failed")?;

        let choice = response
            .choices
            .into_iter()
            .next()
            .context("No choices in LLM response")?;

        Ok((choice.message, response.usage))
    }

    /// Call LLM with streaming, forwarding content deltas to the callback.
    async fn call_llm_streaming(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolSchema]>,
    ) -> Result<(ChatMessage, Option<Usage>)> {
        let mut rx = self
            .client
            .chat_completion_stream(messages, tools, None, None, None, None)
            .await
            .context("Streaming LLM call failed")?;

        let mut content = String::new();
        let mut tool_call_deltas: Vec<ToolCallDelta> = Vec::new();
        let mut usage = None;

        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::ContentDelta(delta) => {
                    if let Some(ref cb) = self.stream_callback {
                        cb(&delta);
                    }
                    content.push_str(&delta);
                }
                StreamEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments,
                } => {
                    tool_call_deltas.push((index, id, name, arguments));
                }
                StreamEvent::Usage(u) => {
                    usage = Some(u);
                }
                StreamEvent::Done(_) => break,
            }
        }

        // Assemble the message
        let tool_calls = if tool_call_deltas.is_empty() {
            None
        } else {
            Some(crate::client::assemble_tool_calls_from_stream(
                &tool_call_deltas,
            ))
        };

        let message = ChatMessage {
            role: ironhermes_core::Role::Assistant,
            content: if content.is_empty() {
                None
            } else {
                Some(ironhermes_core::MessageContent::Text(content))
            },
            tool_calls,
            tool_call_id: None,
            name: None,
        };

        Ok((message, usage))
    }

    /// Execute a single tool call and return the result string.
    ///
    /// 07.4 D-05 ordering:
    ///   1. Check guardrails FIRST (no execution yet).
    ///   2. On Block: fire ToolCompleted{success:false, result_preview=<formatted block error>}.
    ///      Do NOT fire ToolCalled. Return the formatted block error as the tool result so
    ///      the LLM sees the same error string it saw pre-07.4.
    ///   3. On Allow or Warn: fire ToolCalled, then call registry.execute_tool(), then fire
    ///      ToolCompleted with success/failure based on the execution result.
    ///
    /// Warn counts as Allow for event firing (D-08): the tool executes and both hook events
    /// fire. The tracing::warn! side-effect is owned by ToolRegistry::check_guardrails.
    async fn execute_tool_call(&self, tool_call: &ToolCall) -> String {
        use ironhermes_hooks::GuardrailDecision;

        let name = &tool_call.function.name;
        let args_str = &tool_call.function.arguments;

        if let Some(ref cb) = self.tool_progress_callback {
            let preview = if args_str.len() > 100 {
                let mut end = 100;
                while !args_str.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}...", &args_str[..end])
            } else {
                args_str.clone()
            };
            cb(name, &preview);
        }

        debug!(tool = %name, "Executing tool call");

        // Parse args BEFORE firing any hook (pre-07.4 behavior: bad args short-circuits
        // with the same error message and does NOT fire ToolCalled/ToolCompleted).
        let args: serde_json::Value = match serde_json::from_str(args_str) {
            Ok(v) => v,
            Err(e) => {
                let err_msg = format!("Failed to parse tool arguments: {}", e);
                warn!(tool = %name, error = %err_msg);
                return err_msg;
            }
        };

        // SKILL-06 / D-04..D-09: enforce allowed_tools from active skills
        {
            let skills = self.active_skills.lock().unwrap_or_else(|e| e.into_inner());
            // D-04: only enforce when at least one skill has allowed_tools
            let restricting_skills: Vec<&ironhermes_core::SkillRecord> = skills
                .iter()
                .filter(|s| s.allowed_tools.is_some())
                .collect();

            if !restricting_skills.is_empty() {
                // D-05: union of all allowed_tools lists
                let mut allowed: std::collections::HashSet<&str> = std::collections::HashSet::new();
                for skill in &restricting_skills {
                    if let Some(ref tools) = skill.allowed_tools {
                        for t in tools {
                            allowed.insert(t.as_str());
                        }
                    }
                }
                // D-07: skills tool is always permitted
                allowed.insert("skills");

                if !allowed.contains(name.as_str()) {
                    // D-09: actionable error message
                    let mut allowed_list: Vec<&str> = allowed.into_iter().collect();
                    allowed_list.sort();
                    let err_msg = format!(
                        "Tool '{}' is not permitted by the active skill set. Allowed tools: [{}]. \
                         Activate a skill that permits '{}' or deactivate the restricting skill.",
                        name,
                        allowed_list.join(", "),
                        name,
                    );
                    warn!(tool = %name, "Skill enforcement blocked tool call");

                    // D-08: same pattern as guardrail block — fire ToolCompleted{success:false},
                    // do NOT fire ToolCalled
                    self.fire_hook(HookEventKind::ToolCompleted {
                        tool_name: name.to_string(),
                        success: false,
                        result_preview: ironhermes_hooks::event::preview(&err_msg, 200),
                        duration_ms: 0,
                    });

                    return err_msg;
                }
            }
        }

        // D-05 Step 1: check guardrails WITHOUT executing the tool.
        let decision = self.registry.check_guardrails(name, &args);

        match decision {
            GuardrailDecision::Block { reason } => {
                // D-05 / D-07: do NOT fire ToolCalled. Format the error via the same
                // format_guardrail_error path that ToolRegistry::dispatch uses, so the
                // block error respects ErrorDetailLevel and looks identical to the
                // pre-07.4 tool_result string that the LLM sees.
                let err_msg = ironhermes_hooks::format_guardrail_error(
                    name,
                    &reason,
                    "guardrail",
                    self.registry.guardrail_error_detail(),
                );
                warn!(tool = %name, "Tool blocked by guardrail: {}", err_msg);

                // D-05 Step 2: fire ToolCompleted ONLY (no ToolCalled before it).
                self.fire_hook(HookEventKind::ToolCompleted {
                    tool_name: name.to_string(),
                    success: false,
                    result_preview: ironhermes_hooks::event::preview(&err_msg, 200),
                    duration_ms: 0,
                });

                // Return the formatted error as the tool_result so the LLM sees the
                // same error-shaped string it saw pre-07.4.
                err_msg
            }
            GuardrailDecision::Allow | GuardrailDecision::Warn { .. } => {
                // D-05 Step 3: fire ToolCalled FIRST (this is the post-fix ordering).
                // D-08: Warn counts as Allow for event firing — do not skip ToolCalled.
                self.fire_hook(HookEventKind::ToolCalled {
                    tool_name: name.to_string(),
                    args_preview: ironhermes_hooks::event::preview(args_str, 200),
                });

                // D-07: Intercept session_search before registry dispatch.
                // StateStore uses sync rusqlite; wrap in spawn_blocking to avoid blocking tokio.
                if name == "session_search" {
                    if let Some(ref state) = self.state_store {
                        let state_clone = state.clone();
                        let args_clone = args.clone();
                        let result = tokio::task::spawn_blocking(move || {
                            let store = state_clone.lock().unwrap();
                            crate::session_search::handle_session_search(&args_clone, &store)
                        })
                        .await;
                        return match result {
                            Ok(s) => s,
                            Err(e) => format!(
                                r#"{{"error":"internal","reason":"{}"}}"#,
                                e.to_string().replace('"', "'")
                            ),
                        };
                    }
                    return r#"{"error":"unavailable","reason":"state store not configured"}"#.to_string();
                }

                // Save path for subdirectory discovery before args is moved
                let tool_path_arg = args.get("path").and_then(|v| v.as_str()).map(String::from);

                let tool_start = std::time::Instant::now();
                let dispatch_result = self.registry.execute_tool(name, args).await;
                let duration_ms = tool_start.elapsed().as_millis() as u64;

                match dispatch_result {
                    Ok(result) => {
                        self.fire_hook(HookEventKind::ToolCompleted {
                            tool_name: name.to_string(),
                            success: true,
                            result_preview: ironhermes_hooks::event::preview(&result, 200),
                            duration_ms,
                        });

                        // CTX-03/CTX-04: progressive subdirectory discovery for file-access tools
                        let mut final_result = result;
                        const FILE_ACCESS_TOOLS: &[&str] = &["read_file", "write_file", "patch", "search_files"];
                        if FILE_ACCESS_TOOLS.contains(&name.as_str()) {
                            if let Some(ref disc) = self.subdir_discovery {
                                if let Some(ref path_str) = tool_path_arg {
                                    let path = std::path::Path::new(path_str);
                                    if let Ok(mut discovery) = disc.lock() {
                                        if let Some(ctx) = discovery.check_path(path) {
                                            debug!(tool = %name, path = %path_str, "Subdirectory context discovered");
                                            final_result.push_str(&ctx);
                                        }
                                    }
                                }
                            }
                        }
                        final_result
                    }
                    Err(e) => {
                        let err_msg = format!("Tool '{}' failed: {}", name, e);
                        warn!(%err_msg);
                        self.fire_hook(HookEventKind::ToolCompleted {
                            tool_name: name.to_string(),
                            success: false,
                            result_preview: ironhermes_hooks::event::preview(&err_msg, 200),
                            duration_ms,
                        });
                        err_msg
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests: hook ordering and duplicate-event prevention (07.4-02)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod hooks_ordering_tests {
    use super::*;
    use async_trait::async_trait;
    use ironhermes_core::ToolSchema;
    use ironhermes_hooks::{
        BlocklistGuardrail, GuardrailDecision, GuardrailHook, HookEvent, HookEventKind,
        HookRegistry, HooksConfig,
    };
    use ironhermes_tools::{Tool, ToolRegistry};
    use std::sync::{Arc, Mutex};

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn capture_registry() -> (Arc<HookRegistry>, Arc<Mutex<Vec<HookEvent>>>) {
        let mut registry = HookRegistry::new(HooksConfig::default());
        let captured: Arc<Mutex<Vec<HookEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let cap = captured.clone();
        registry.add_listener(Arc::new(move |event: HookEvent| {
            cap.lock().unwrap().push(event);
        }));
        (Arc::new(registry), captured)
    }

    // -----------------------------------------------------------------------
    // Mock tools
    // -----------------------------------------------------------------------

    struct OkMockTool;

    #[async_trait]
    impl Tool for OkMockTool {
        fn name(&self) -> &str {
            "mock"
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "ok mock"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "mock",
                "ok mock",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("mock result".to_string())
        }
    }

    struct FailMockTool;

    #[async_trait]
    impl Tool for FailMockTool {
        fn name(&self) -> &str {
            "failmock"
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "fail mock"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "failmock",
                "fail mock",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Err(anyhow::anyhow!("boom"))
        }
    }

    // -----------------------------------------------------------------------
    // Warn guardrail
    // -----------------------------------------------------------------------

    struct WarnGuardrail;

    impl GuardrailHook for WarnGuardrail {
        fn check(&self, _name: &str, _args: &serde_json::Value) -> GuardrailDecision {
            GuardrailDecision::Warn {
                reason: "always warn".to_string(),
            }
        }
        fn name(&self) -> &str {
            "warn-always"
        }
    }

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn tool_call(name: &str) -> ToolCall {
        ToolCall {
            id: "call-1".to_string(),
            call_type: "function".to_string(),
            function: ironhermes_core::FunctionCall {
                name: name.to_string(),
                arguments: "{}".to_string(),
            },
        }
    }

    fn build_agent(tool_registry: ToolRegistry, hook_registry: Arc<HookRegistry>) -> AgentLoop {
        let client = AnyClient::ChatCompletions(
            crate::client::LlmClient::new("http://localhost".to_string(), "".to_string(), "mock-model"),
        );
        AgentLoop::new(client, Arc::new(tool_registry), 4).with_hook_registry(hook_registry)
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    /// D-05 / D-07 / audit warning #3: a blocked tool must emit zero ToolCalled
    /// and exactly one ToolCompleted{success:false} whose result_preview contains
    /// the block reason.
    #[tokio::test]
    async fn test_blocked_tool_no_tool_called_event() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool));
        tool_registry
            .add_guardrail(Box::new(BlocklistGuardrail::new(vec!["mock".to_string()])));

        let (hook_registry, captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        let result = agent.execute_tool_call(&tool_call("mock")).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let events = captured.lock().unwrap();
        let tool_called_count = events
            .iter()
            .filter(|e| matches!(e.kind, HookEventKind::ToolCalled { .. }))
            .count();
        let tool_completed: Vec<_> = events
            .iter()
            .filter_map(|e| match &e.kind {
                HookEventKind::ToolCompleted {
                    success,
                    result_preview,
                    ..
                } => Some((*success, result_preview.clone())),
                _ => None,
            })
            .collect();

        assert_eq!(
            tool_called_count, 0,
            "blocked tool must not emit ToolCalled (audit warning #3)"
        );
        assert_eq!(
            tool_completed.len(),
            1,
            "blocked tool must emit exactly one ToolCompleted"
        );
        assert_eq!(
            tool_completed[0].0, false,
            "blocked tool ToolCompleted must have success=false"
        );
        assert!(
            tool_completed[0].1.contains("blocked")
                || tool_completed[0].1.contains("security policy")
                || tool_completed[0].1.contains("blocklist"),
            "ToolCompleted.result_preview must contain block reason: {:?}",
            tool_completed[0].1
        );
        assert!(
            result.contains("blocked")
                || result.contains("security policy")
                || result.contains("blocklist"),
            "tool_result returned to LLM must be the formatted block error: {result}"
        );
    }

    /// Allowed tool fires ToolCalled then ToolCompleted{success:true} in order.
    #[tokio::test]
    async fn test_allowed_tool_fires_tool_called_then_tool_completed() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool));
        let (hook_registry, captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        let result = agent.execute_tool_call(&tool_call("mock")).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(result, "mock result");
        let events = captured.lock().unwrap();
        assert_eq!(
            events.len(),
            2,
            "expected ToolCalled + ToolCompleted, got {:?}",
            *events
        );
        assert!(
            matches!(events[0].kind, HookEventKind::ToolCalled { .. }),
            "first event must be ToolCalled"
        );
        assert!(
            matches!(
                events[1].kind,
                HookEventKind::ToolCompleted { success: true, .. }
            ),
            "second event must be ToolCompleted{{success:true}}"
        );
    }

    /// D-08: warn counts as allow for event firing — ToolCalled + ToolCompleted both fire.
    #[tokio::test]
    async fn test_warn_guardrail_fires_tool_called_and_tool_completed() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool));
        tool_registry.add_guardrail(Box::new(WarnGuardrail));
        let (hook_registry, captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        let _ = agent.execute_tool_call(&tool_call("mock")).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let events = captured.lock().unwrap();
        assert_eq!(
            events.len(),
            2,
            "warn must still emit ToolCalled + ToolCompleted"
        );
        assert!(matches!(events[0].kind, HookEventKind::ToolCalled { .. }));
        assert!(matches!(
            events[1].kind,
            HookEventKind::ToolCompleted { success: true, .. }
        ));
    }

    /// Execution errors on an allowed tool still emit ToolCalled + ToolCompleted{success:false}.
    #[tokio::test]
    async fn test_allowed_tool_execution_failure_still_fires_tool_called() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(FailMockTool));
        let (hook_registry, captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        let _ = agent.execute_tool_call(&tool_call("failmock")).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let events = captured.lock().unwrap();
        assert_eq!(
            events.len(),
            2,
            "failed allowed tool must still emit both events"
        );
        assert!(matches!(events[0].kind, HookEventKind::ToolCalled { .. }));
        assert!(matches!(
            events[1].kind,
            HookEventKind::ToolCompleted { success: false, .. }
        ));
    }

    /// D-01 / audit warning #4: agent_loop.rs must not fire MessageReceived or ResponseSent.
    /// Uses include_str! as a compile-time regression guard — future edits that reintroduce
    /// these forbidden fires will trip this test without needing a mock LlmClient.
    ///
    /// We search for the fire_hook call patterns specifically so the assertion strings
    /// themselves (which mention the type names for documentation purposes) do not
    /// cause false positives.
    // -----------------------------------------------------------------------
    // Skill enforcement tests (SKILL-06 / 07.5-01 Task 2)
    // -----------------------------------------------------------------------

    fn make_skill_record(name: &str, allowed_tools: Option<Vec<&str>>) -> ironhermes_core::SkillRecord {
        ironhermes_core::SkillRecord {
            name: name.to_string(),
            description: format!("{} skill", name),
            path: std::path::PathBuf::from("/tmp/fake"),
            platforms: None,
            compatibility: None,
            allowed_tools: allowed_tools.map(|v| v.into_iter().map(|s| s.to_string()).collect()),
            metadata: None,
        }
    }

    #[tokio::test]
    async fn test_skill_enforcement_blocks_unlisted_tool() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool));
        let (hook_registry, _captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        // Pre-populate active_skills with a restrictive skill
        {
            let mut skills = agent.active_skills.lock().unwrap();
            skills.push(make_skill_record("focus", Some(vec!["web_read"])));
        }

        let result = agent.execute_tool_call(&tool_call("mock")).await;
        assert!(
            result.contains("not permitted by the active skill set"),
            "blocked tool should get enforcement error, got: {result}"
        );
        assert!(
            result.contains("Allowed tools"),
            "error should list allowed tools, got: {result}"
        );
    }

    #[tokio::test]
    async fn test_skill_enforcement_allows_listed_tool() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool));
        let (hook_registry, _captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        {
            let mut skills = agent.active_skills.lock().unwrap();
            skills.push(make_skill_record("focus", Some(vec!["mock"])));
        }

        let result = agent.execute_tool_call(&tool_call("mock")).await;
        assert_eq!(result, "mock result", "listed tool should execute normally");
    }

    #[tokio::test]
    async fn test_skill_enforcement_inactive_means_all_allowed() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool));
        let (hook_registry, _captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        // No active skills — everything is allowed
        let result = agent.execute_tool_call(&tool_call("mock")).await;
        assert_eq!(result, "mock result", "no active skills = all tools allowed");
    }

    #[tokio::test]
    async fn test_skill_enforcement_skills_tool_always_allowed() {
        let mut tool_registry = ToolRegistry::new();
        // Register a mock tool named "skills" to simulate the skills tool
        struct SkillsMockTool;
        #[async_trait]
        impl Tool for SkillsMockTool {
            fn name(&self) -> &str { "skills" }
            fn toolset(&self) -> &str { "test" }
            fn description(&self) -> &str { "mock skills" }
            fn schema(&self) -> ToolSchema {
                ToolSchema::new("skills", "mock skills", serde_json::json!({"type": "object", "properties": {}}))
            }
            async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
                Ok("skills result".to_string())
            }
        }
        tool_registry.register(Box::new(SkillsMockTool));
        let (hook_registry, _captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        {
            let mut skills = agent.active_skills.lock().unwrap();
            skills.push(make_skill_record("focus", Some(vec!["web_read"])));
        }

        let result = agent.execute_tool_call(&tool_call("skills")).await;
        assert_eq!(result, "skills result", "skills tool must always be permitted (D-07)");
    }

    #[tokio::test]
    async fn test_skill_enforcement_union_of_multiple_skills() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool)); // name = "mock"
        let (hook_registry, _captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        {
            let mut skills = agent.active_skills.lock().unwrap();
            skills.push(make_skill_record("skill-a", Some(vec!["web_read"])));
            skills.push(make_skill_record("skill-b", Some(vec!["memory"])));
        }

        // "mock" is not in the union {web_read, memory, skills} -> should be blocked
        let result = agent.execute_tool_call(&tool_call("mock")).await;
        assert!(
            result.contains("not permitted"),
            "tool not in union should be blocked, got: {result}"
        );
    }

    #[tokio::test]
    async fn test_skill_enforcement_none_allowed_tools_ignored() {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(OkMockTool));
        let (hook_registry, _captured) = capture_registry();
        let agent = build_agent(tool_registry, hook_registry);

        {
            let mut skills = agent.active_skills.lock().unwrap();
            // Skill with None allowed_tools — does not restrict (D-06)
            skills.push(make_skill_record("non-restricting", None));
            // Skill with Some allowed_tools — restricts to web_read only
            skills.push(make_skill_record("restricting", Some(vec!["web_read"])));
        }

        // "mock" is not in allowed set -> blocked because the restricting skill is active
        let result = agent.execute_tool_call(&tool_call("mock")).await;
        assert!(
            result.contains("not permitted"),
            "tool should be blocked when any skill restricts, got: {result}"
        );
    }

    // -----------------------------------------------------------------------
    // CancellationToken tests (09-04 Task 1)
    // -----------------------------------------------------------------------

    #[test]
    fn test_agent_loop_with_cancellation_token_sets_token() {
        use tokio_util::sync::CancellationToken;
        let client = AnyClient::ChatCompletions(
            crate::client::LlmClient::new("http://localhost".to_string(), "".to_string(), "mock-model"),
        );
        let registry = Arc::new(ToolRegistry::new());
        let token = CancellationToken::new();
        let agent = AgentLoop::new(client, registry, 4)
            .with_cancellation_token(token.clone());
        // Verify the token is set (it exists on the struct)
        assert!(agent.cancel_token.is_some(), "cancel_token should be set after with_cancellation_token");
    }

    #[tokio::test]
    async fn test_agent_loop_run_returns_early_when_cancelled_before_first_iteration() {
        use tokio_util::sync::CancellationToken;
        let client = AnyClient::ChatCompletions(
            crate::client::LlmClient::new("http://localhost".to_string(), "".to_string(), "mock-model"),
        );
        let registry = Arc::new(ToolRegistry::new());
        let token = CancellationToken::new();
        // Cancel BEFORE run
        token.cancel();
        let mut agent = AgentLoop::new(client, registry, 4)
            .with_cancellation_token(token);
        let messages = vec![ChatMessage::user("hello")];
        let result = agent.run(messages).await.unwrap();
        assert!(!result.finished_naturally, "should not finish naturally when cancelled");
        assert_eq!(result.final_response.as_deref(), Some("Cancelled by parent"));
        assert_eq!(result.turns_used, 0, "should use 0 turns when cancelled before first iteration");
    }

    #[tokio::test]
    async fn test_agent_loop_source_has_no_message_received_or_response_sent_fires() {
        let src = include_str!("agent_loop.rs");
        // Search for the actual fire_hook invocation patterns, not bare type references.
        // The assertion strings below intentionally avoid containing these exact patterns.
        let msg_rcvd_fire = concat!("fire_hook(HookEventKind::", "MessageReceived");
        let resp_sent_fire = concat!("fire_hook(HookEventKind::", "ResponseSent");
        assert!(
            !src.contains(msg_rcvd_fire),
            "agent_loop.rs must not call fire_hook for MessageReceived (D-01, audit warning #4)"
        );
        assert!(
            !src.contains(resp_sent_fire),
            "agent_loop.rs must not call fire_hook for ResponseSent (D-01, audit warning #4)"
        );
    }
}

// ---------------------------------------------------------------------------
// Tests: iteration budget (12-03 PROV-09, PROV-10)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod budget_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    fn make_agent(max_iterations: usize) -> AgentLoop {
        let client = AnyClient::ChatCompletions(
            crate::client::LlmClient::new("http://localhost".to_string(), "".to_string(), "test"),
        );
        let registry = Arc::new(ironhermes_tools::ToolRegistry::new());
        AgentLoop::new(client, registry, max_iterations)
    }

    #[test]
    fn test_budget_threshold_below_70() {
        let budget = Arc::new(AtomicUsize::new(5));
        let agent = make_agent(10).with_budget(budget);
        // 5/10 = 50% — below 70%
        assert_eq!(agent.check_budget_threshold(), None);
    }

    #[test]
    fn test_budget_threshold_at_70() {
        let budget = Arc::new(AtomicUsize::new(7));
        let agent = make_agent(10).with_budget(budget);
        let result = agent.check_budget_threshold();
        assert!(result.is_some(), "expected Some at 70%");
        assert!(result.unwrap().contains("[Caution]"), "expected [Caution] at 70%");
    }

    #[test]
    fn test_budget_threshold_at_90() {
        let budget = Arc::new(AtomicUsize::new(9));
        let agent = make_agent(10).with_budget(budget);
        let result = agent.check_budget_threshold();
        assert!(result.is_some(), "expected Some at 90%");
        assert!(result.unwrap().contains("[Warning]"), "expected [Warning] at 90%");
    }

    #[test]
    fn test_shared_budget_increment() {
        let budget = Arc::new(AtomicUsize::new(0));
        let parent_budget = budget.clone();
        let child_budget = budget.clone();
        for _ in 0..5 {
            parent_budget.fetch_add(1, Ordering::SeqCst);
        }
        for _ in 0..3 {
            child_budget.fetch_add(1, Ordering::SeqCst);
        }
        assert_eq!(budget.load(Ordering::SeqCst), 8);
    }

    #[test]
    fn test_budget_getter_returns_arc() {
        let budget = Arc::new(AtomicUsize::new(0));
        let agent = make_agent(10).with_budget(budget.clone());
        let retrieved = agent.budget();
        assert!(retrieved.is_some(), "budget() should return Some after with_budget");
        retrieved.unwrap().fetch_add(1, Ordering::SeqCst);
        assert_eq!(budget.load(Ordering::SeqCst), 1);
    }
}

// ---------------------------------------------------------------------------
// Tests: one-shot fallback (12-03 PROV-07, D-11)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod fallback_tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn test_fallback_state_initial() {
        let client = AnyClient::ChatCompletions(
            crate::client::LlmClient::new("http://localhost".to_string(), "".to_string(), "test"),
        );
        let registry = Arc::new(ironhermes_tools::ToolRegistry::new());
        let agent = AgentLoop::new(client, registry, 10);
        assert!(!agent.fallback_activated, "fallback_activated should start false");
        assert!(agent.fallback_client.is_none(), "fallback_client should start None");
    }

    #[test]
    fn test_classify_429_error() {
        let err = anyhow!("HTTP request failed with status: 429 Too Many Requests");
        let (should_retry, should_fallback) = AgentLoop::classify_llm_error(&err);
        assert!(should_retry, "429 should be retryable");
        assert!(should_fallback, "429 should trigger fallback");
    }

    #[test]
    fn test_classify_401_error() {
        let err = anyhow!("HTTP request failed with status: 401 Unauthorized");
        let (should_retry, should_fallback) = AgentLoop::classify_llm_error(&err);
        assert!(!should_retry, "401 should not be retried");
        assert!(should_fallback, "401 should trigger fallback");
    }

    #[test]
    fn test_classify_other_error() {
        let err = anyhow!("Connection refused: failed to connect to LLM");
        let (should_retry, should_fallback) = AgentLoop::classify_llm_error(&err);
        assert!(should_retry, "generic errors should be retryable");
        assert!(!should_fallback, "generic errors should not trigger fallback");
    }

    #[test]
    fn test_fallback_activated_prevents_refire() {
        let primary = AnyClient::ChatCompletions(
            crate::client::LlmClient::new("http://primary".to_string(), "key1".to_string(), "model1"),
        );
        let fallback = AnyClient::ChatCompletions(
            crate::client::LlmClient::new("http://fallback".to_string(), "key2".to_string(), "model2"),
        );
        let registry = Arc::new(ironhermes_tools::ToolRegistry::new());
        let mut agent = AgentLoop::new(primary, registry, 10).with_fallback(fallback);

        assert!(!agent.fallback_activated);
        assert!(agent.fallback_client.is_some());

        // Manually activate fallback (as the run() loop would)
        if let Some(fb) = agent.fallback_client.take() {
            agent.client = fb;
            agent.fallback_activated = true;
        }

        assert!(agent.fallback_activated);
        assert!(agent.fallback_client.is_none(), "take() should leave None — one-shot guarantee");
    }

    #[test]
    fn test_classify_500_error() {
        let err = anyhow!("HTTP request failed with status: 500 Internal Server Error");
        let (should_retry, should_fallback) = AgentLoop::classify_llm_error(&err);
        assert!(should_retry, "500 should be retryable");
        assert!(should_fallback, "500 should trigger fallback");
    }
}
