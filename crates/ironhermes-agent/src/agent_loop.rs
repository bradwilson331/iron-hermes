use anyhow::{Context, Result};
use ironhermes_core::{ChatMessage, ChatResponse, ToolCall, ToolSchema, Usage};
use ironhermes_tools::ToolRegistry;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::client::{LlmClient, StreamEvent};
use crate::context_compressor::ContextCompressor;

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
    client: LlmClient,
    registry: Arc<ToolRegistry>,
    max_iterations: usize,
    compressor: Option<Mutex<ContextCompressor>>,
    stream_callback: Option<StreamCallback>,
    tool_progress_callback: Option<ToolProgressCallback>,
    streaming: bool,
}

impl AgentLoop {
    pub fn new(client: LlmClient, registry: Arc<ToolRegistry>, max_iterations: usize) -> Self {
        Self {
            client,
            registry,
            max_iterations,
            compressor: None,
            stream_callback: None,
            tool_progress_callback: None,
            streaming: false,
        }
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

    /// Run the agent loop with the given messages.
    ///
    /// The loop continues until:
    /// - The LLM produces a response with no tool calls (natural completion)
    /// - Max iterations are reached
    /// - An unrecoverable error occurs
    pub async fn run(&self, mut messages: Vec<ChatMessage>) -> Result<AgentResult> {
        let tool_schemas = self.registry.get_definitions(None);
        let tools_option = if tool_schemas.is_empty() {
            None
        } else {
            Some(tool_schemas)
        };

        let mut turns_used = 0;
        let mut total_usage = AggregatedUsage::default();
        let mut final_response = None;

        info!(max_iterations = self.max_iterations, "Starting agent loop");

        loop {
            if turns_used >= self.max_iterations {
                warn!(turns = turns_used, "Max iterations reached");
                break;
            }

            // Check for context compression
            if let Some(ref compressor) = self.compressor {
                let mut comp = compressor.lock().await;
                comp.compress(&mut messages);
            }

            turns_used += 1;
            debug!(turn = turns_used, messages = messages.len(), "Agent loop turn");

            // Call LLM
            let (assistant_message, usage) = if self.streaming {
                self.call_llm_streaming(&messages, tools_option.as_deref())
                    .await?
            } else {
                self.call_llm(&messages, tools_option.as_deref()).await?
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
            if let Some(text) = assistant_message.content_text() {
                if !text.is_empty() {
                    final_response = Some(text.to_string());
                }
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
        let mut tool_call_deltas: Vec<(usize, Option<String>, Option<String>, Option<String>)> =
            Vec::new();
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
    async fn execute_tool_call(&self, tool_call: &ToolCall) -> String {
        let name = &tool_call.function.name;
        let args_str = &tool_call.function.arguments;

        if let Some(ref cb) = self.tool_progress_callback {
            let preview = if args_str.len() > 100 {
                format!("{}...", &args_str[..100])
            } else {
                args_str.clone()
            };
            cb(name, &preview);
        }

        debug!(tool = %name, "Executing tool call");

        let args: serde_json::Value = match serde_json::from_str(args_str) {
            Ok(v) => v,
            Err(e) => {
                let err_msg = format!("Failed to parse tool arguments: {}", e);
                warn!(tool = %name, error = %err_msg);
                return err_msg;
            }
        };

        match self.registry.dispatch(name, args).await {
            Ok(result) => result,
            Err(e) => {
                let err_msg = format!("Tool '{}' failed: {}", name, e);
                warn!(%err_msg);
                err_msg
            }
        }
    }
}
