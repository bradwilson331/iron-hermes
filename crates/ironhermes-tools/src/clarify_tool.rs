// Phase 36.3.8 Plan 02 — ClarifyTool + ClarifyDispatcher trait.
//
// `ClarifyDispatcher` trait lives here (in ironhermes-tools) so that gateway can
// implement it for `TelegramAdapter` without a circular crate dependency
// (gateway → tools, not the reverse). Per OQ-3 / D-04, ClarifyDispatcher is a
// SEPARATE trait from MessageDispatcher (SOLID — each tool depends only on the
// methods it needs).
//
// D-04: ClarifyDispatcher trait defined here; Telegram impl in gateway (Plan 03).
// D-05: Blocking bounded suspend via oneshot + tokio::select!.
// D-06: select! integrates 39.1 CancellationToken + mandatory timeout.
// D-07: sentinel results {"answered":false,"reason":"timeout"|"cancelled"}.
//
// WASM-safety: clarify_dispatcher and cancel_token are Option so the tool
// can be registered inert (is_available → false) on wasm32 where the gateway
// injected values are None.

use async_trait::async_trait;
use std::sync::Arc;

use crate::clarify_registry::{ClarifyAnswer, PendingClarifyRegistry};

// ---------------------------------------------------------------------------
// Public const: callback_data prefix grammar
// ---------------------------------------------------------------------------

/// Prefix for all clarify callback_data strings.
///
/// Full grammar: `"clarify:<clarify_id>:<choice_index>"`
///
/// Plan 03's gateway runner imports this const so the encoding and parsing
/// always use the same token (no string literal duplication).
pub const CLARIFY_CALLBACK_PREFIX: &str = "clarify";

// ---------------------------------------------------------------------------
// ClarifyDispatcher trait — D-04 (orphan-rule-safe, SOLID — OQ-3)
// ---------------------------------------------------------------------------

/// Platform-agnostic clarify-question delivery abstraction for `clarify`.
///
/// Implemented in `ironhermes-gateway` for `TelegramAdapter` (orphan-rule-safe:
/// trait defined here in tools, type defined there in gateway).
///
/// This trait is intentionally SEPARATE from `MessageDispatcher` (OQ-3 / SOLID):
/// `ClarifyTool` only depends on the one method it needs, and there is no
/// supertrait relationship between the two dispatcher traits.
#[async_trait]
pub trait ClarifyDispatcher: Send + Sync {
    /// Send a question with button choices to the session's chat.
    ///
    /// For Telegram (Plan 03): sends `sendMessage` with `reply_markup.inline_keyboard`.
    /// For local (CLI/TUI): prints a numbered list to stdout as a text fallback.
    ///
    /// `clarify_id` is embedded in each button's `callback_data` using the
    /// `clarify:<clarify_id>:<choice_index>` grammar so the gateway can route
    /// the callback_query back to the correct pending awaiter.
    async fn send_question(
        &self,
        chat_id: &str,
        thread_id: Option<&str>,
        question: &str,
        choices: &[String],
        clarify_id: &str,
    ) -> anyhow::Result<()>;
}

// ---------------------------------------------------------------------------
// ClarifyTool struct
// ---------------------------------------------------------------------------

/// LLM-callable tool that suspends the current turn until the user answers a
/// question with one of the provided choices.
///
/// Suspend/resume mechanics:
/// 1. Inserts `(choices, oneshot::Sender)` into `PendingClarifyRegistry` keyed
///    by `clarify_id` (derived from `session_key.chat_id` — compact, routeable).
/// 2. Dispatches the question to the platform via `ClarifyDispatcher`.
/// 3. `tokio::select!` over three arms:
///    - oneshot receiver (answered by human)
///    - `tokio::time::sleep(clarify_timeout_secs)` (D-06 mandatory timeout)
///    - `cancel_token.cancelled()` (D-06 / 39.1 `/stop` / `/stop-all`)
/// 4. Always removes the registry entry before returning (Pitfall 5).
/// 5. Returns the D-07 sentinel on timeout/cancel.
pub struct ClarifyTool {
    session_key: ironhermes_core::SessionKey,
    clarify_dispatcher: Option<Arc<dyn ClarifyDispatcher>>,
    clarify_registry: Arc<PendingClarifyRegistry>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    config: Arc<ironhermes_core::Config>,
}

impl ClarifyTool {
    pub fn new(
        session_key: ironhermes_core::SessionKey,
        clarify_dispatcher: Option<Arc<dyn ClarifyDispatcher>>,
        clarify_registry: Arc<PendingClarifyRegistry>,
        cancel_token: Option<tokio_util::sync::CancellationToken>,
        config: Arc<ironhermes_core::Config>,
    ) -> Self {
        Self {
            session_key,
            clarify_dispatcher,
            clarify_registry,
            cancel_token,
            config,
        }
    }

    /// Whether a clarify delivery channel is available for this tool's platform.
    ///
    /// Phase 48.2 Plan 10 (D-16/G-48.2-3): single source of truth shared by
    /// `Tool::is_available()` and `Tool::prerequisites()` so the two can never
    /// silently disagree — `is_available()` reports the same condition
    /// `prerequisites()` describes the cause of, by construction.
    ///
    /// Local platform is always available (the text fallback works without a
    /// dispatcher). WR-04: Web sessions wire `clarify_dispatcher: None` and have
    /// no callback-query resolver yet, so the tool is inert on Web until a
    /// WS-based resolver exists. Every other platform needs an attached
    /// dispatcher.
    fn clarify_channel_available(&self) -> bool {
        match self.session_key.platform {
            ironhermes_core::Platform::Local => true,
            ironhermes_core::Platform::Web => self.clarify_dispatcher.is_some(),
            _ => self.clarify_dispatcher.is_some(),
        }
    }

    /// The core suspend/resume logic, exposed as a named method so tests can
    /// call it directly without going through the `Tool` trait's `execute()`.
    ///
    /// This method always removes the registry entry before returning (Pitfall 5).
    pub async fn execute_clarify(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let question = args["question"].as_str().unwrap_or("?");
        let choices: Vec<String> = args["choices"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        if choices.is_empty() {
            anyhow::bail!("clarify: 'choices' must be a non-empty array of strings");
        }

        let timeout_secs = self.config.agent.clarify_timeout_secs;

        // Build the clarify_id from the session chat_id. Using chat_id ensures
        // the gateway can route the callback_query back to the right suspended
        // turn without extra state — the chat is the natural partition key.
        // Format: "clarify:<chat_id>" — unique per session (only one clarify
        // can be active per chat at a time in v1).
        let clarify_id = format!("{}:{}", CLARIFY_CALLBACK_PREFIX, self.session_key.chat_id);

        // 1. Create oneshot pair.
        let (tx, rx) = tokio::sync::oneshot::channel::<ClarifyAnswer>();

        // 2. Insert (choices + sender) into registry BEFORE sending the question
        //    to eliminate the race where a very fast response arrives first.
        //    WR-02: insert returns Err if a clarify is already active for this
        //    chat; propagate so the model receives a clear error instead of a
        //    misleading "reason":"timeout" for the evicted turn.
        self.clarify_registry
            .insert(clarify_id.clone(), choices.clone(), tx)
            .await?;

        // 3. Send question to platform.
        if let Some(dispatcher) = &self.clarify_dispatcher {
            if let Err(e) = dispatcher
                .send_question(
                    &self.session_key.chat_id,
                    None,
                    question,
                    &choices,
                    &clarify_id,
                )
                .await
            {
                // Dispatcher error — clean up registry and propagate.
                self.clarify_registry.remove(&clarify_id).await;
                return Err(e);
            }
        } else {
            // Local fallback (CLI/TUI/web text): print numbered list to stdout.
            println!("[clarify] {}", question);
            for (i, choice) in choices.iter().enumerate() {
                println!("  {}: {}", i, choice);
            }
        }

        // 4. Bounded select! — three arms per D-05/D-06.
        //
        //    cancel_token: use a fresh uncancelled token when None so the
        //    cancelled() future is always available but never fires — this
        //    avoids an Option::unwrap() inside select! and is the WASM-safe
        //    pattern (tokio_util CancellationToken is available on wasm32).
        let cancel = self.cancel_token.clone().unwrap_or_default();
        let timeout_duration = std::time::Duration::from_secs(timeout_secs);

        // Use Option<Result<ClarifyAnswer, &str>> to distinguish the three arms:
        //   Some(Ok(answer))  → answered
        //   Some(Err("cancelled")) → cancelled
        //   None              → timeout (or sender dropped)
        let select_result: Option<Result<ClarifyAnswer, &str>> = tokio::select! {
            res = rx => res.ok().map(Ok),
            _ = tokio::time::sleep(timeout_duration) => None,
            _ = cancel.cancelled() => Some(Err("cancelled")),
        };

        // 5. Always remove registry entry on every exit path (Pitfall 5).
        //    For the Answered path the entry was already taken by the gateway,
        //    but remove() is a no-op when the key is absent, so this is safe.
        self.clarify_registry.remove(&clarify_id).await;

        // 6. Return D-07 sentinels.
        match select_result {
            Some(Ok(answer)) => Ok(serde_json::json!({
                "answered": true,
                "choice": answer.label,
                "choice_index": answer.index
            })
            .to_string()),
            Some(Err(reason)) => Ok(serde_json::json!({
                "answered": false,
                "reason": reason
            })
            .to_string()),
            None => Ok(serde_json::json!({
                "answered": false,
                "reason": "timeout"
            })
            .to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Tool trait impl
// ---------------------------------------------------------------------------

#[async_trait]
impl crate::registry::Tool for ClarifyTool {
    fn name(&self) -> &str {
        "clarify"
    }

    fn toolset(&self) -> &str {
        "messaging"
    }

    fn description(&self) -> &str {
        "Ask the user a question with multiple-choice answers (native buttons on \
         supported platforms). Suspends the current turn until answered, then \
         returns the chosen option as the tool result so reasoning can continue \
         in the same turn. Returns a sentinel on timeout or cancellation."
    }

    fn schema(&self) -> ironhermes_core::ToolSchema {
        ironhermes_core::ToolSchema::new(
            "clarify",
            "Ask the user to choose one of the provided options; returns the chosen label.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "The question to present to the user."
                    },
                    "choices": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "List of option labels for the user to choose from \
                                        (2–10 recommended). Presented as native buttons on \
                                        Telegram; as a numbered list on CLI/TUI."
                    }
                },
                "required": ["question", "choices"]
            }),
        )
    }

    /// Returns false when no dispatcher is attached AND the platform requires
    /// one (i.e. Telegram). Local platform is always available — the text
    /// fallback (stdout) works without a dispatcher.
    ///
    /// WR-04: Web sessions wire `clarify_dispatcher: None` and have no
    /// callback-query resolver, so every clarify call would stall the turn for
    /// the full `clarify_timeout_secs`. Return `false` on Web until a WS-based
    /// resolver is implemented, so the tool is not advertised to the LLM.
    ///
    /// Delegates to `clarify_channel_available()` — see that method's doc
    /// comment for why this must stay the single source of truth shared with
    /// `prerequisites()` (D-09/D-16, Phase 48.2 Plan 10).
    fn is_available(&self) -> bool {
        self.clarify_channel_available()
    }

    /// D-09 (Phase 48.2 Plan 10, closing G-48.2-3): declares the real cause when
    /// `is_available()` is false, so `unsatisfied_prerequisites()` — and the Tools
    /// page card it feeds — has something to show instead of `missing: []`.
    ///
    /// Derived from the SAME condition `is_available()` evaluates
    /// (`clarify_channel_available()`), so the two can never drift apart: empty
    /// when available, exactly one `Prerequisite::runtime` entry when not.
    fn prerequisites(&self) -> Vec<crate::registry::Prerequisite> {
        if self.clarify_channel_available() {
            vec![]
        } else {
            vec![crate::registry::Prerequisite::runtime(
                "clarify_channel",
                "This tool needs a clarification channel back to the user. The web \
                 session does not provide one yet — `clarify` works on the CLI.",
            )]
        }
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        self.execute_clarify(args).await
    }
}

// ---------------------------------------------------------------------------
// Callback-data grammar helpers — pub so Plan 03 uses the same encoding
// ---------------------------------------------------------------------------

/// Encode a clarify callback_data string.
///
/// `clarify_id` already carries the `"clarify:<chat_id>"` prefix (constructed
/// by `ClarifyTool::execute_clarify`). This function appends only the choice
/// index, producing `"clarify:<chat_id>:<choice_index>"`.
///
/// Do NOT re-add the `CLARIFY_CALLBACK_PREFIX` here — `clarify_id` is already
/// the full registry key with the prefix included.
///
/// This is the compact format embedded in each Telegram inline-keyboard button.
/// The gateway (Plan 03) calls `parse_clarify_callback` on every incoming
/// `callback_query.data` to route answers back to the correct awaiter.
pub fn clarify_callback_data(clarify_id: &str, choice_index: usize) -> String {
    format!("{}:{}", clarify_id, choice_index)
}

/// Parse a clarify callback_data string.
///
/// Returns `Some((clarify_id, choice_index))` when `data` matches the grammar
/// `"clarify:<chat_id>:<choice_index>"`, `None` otherwise.
///
/// Uses `rsplit_once(':')` so the id (which itself contains a colon, e.g.
/// `"clarify:123456789"`) is preserved intact — the index is always the last
/// colon-delimited segment. The returned `clarify_id` is the exact registry
/// key used by `ClarifyTool::execute_clarify` and `clarify_registry.take()`.
pub fn parse_clarify_callback(data: &str) -> Option<(String, usize)> {
    // Split on the LAST colon: id may contain colons (e.g. "clarify:123456789").
    let (id, idx_str) = data.rsplit_once(':')?;
    if !id.starts_with(CLARIFY_CALLBACK_PREFIX) {
        return None;
    }
    let idx = idx_str.parse::<usize>().ok()?;
    Some((id.to_string(), idx))
}

/// Phase 48.2 Plan 10 (D-16/G-48.2-3): shared test helper — given any `&dyn Tool`,
/// assert that `!is_available()` implies `unsatisfied_prerequisites()` is
/// non-empty. `pub(crate)` (not nested in `mod tests`) so `send_message_tool`'s
/// test module can exercise the SAME assertion against `SendMessageTool` rather
/// than re-implementing it, letting a third tool added later drop into the same
/// check.
#[cfg(test)]
pub(crate) fn assert_unavailable_implies_nonempty_prereqs(tool: &dyn crate::registry::Tool) {
    if !tool.is_available() {
        assert!(
            !crate::registry::unsatisfied_prerequisites(tool).is_empty(),
            "tool '{}' is unavailable but reports zero unsatisfied prerequisites — \
             violates the D-09 contract (registry.rs is_available doc)",
            tool.name()
        );
    }
}

// ---------------------------------------------------------------------------
// Inline unit tests for callback_data helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Tool;

    #[test]
    fn clarify_callback_data_round_trip() {
        let encoded = clarify_callback_data("clarify:my-chat-id", 2);
        let (id, idx) = parse_clarify_callback(&encoded).expect("must parse");
        assert_eq!(id, "clarify:my-chat-id");
        assert_eq!(idx, 2);
    }

    /// Verify the full encode→parse→registry-key chain with a realistic numeric
    /// chat_id. The parsed id must equal the key that `ClarifyTool::execute_clarify`
    /// inserts into the registry (`format!("{}:{}", CLARIFY_CALLBACK_PREFIX, chat_id)`).
    #[test]
    fn clarify_callback_data_round_trip_numeric_chat_id() {
        let chat_id = "123456789";
        // Registry key as constructed by ClarifyTool::execute_clarify:
        let registry_key = format!("{}:{}", CLARIFY_CALLBACK_PREFIX, chat_id);
        // encode → "clarify:123456789:1"
        let encoded = clarify_callback_data(&registry_key, 1);
        assert_eq!(encoded, "clarify:123456789:1");
        // parse → id must equal the registry key so registry.take(&id) finds the entry
        let (parsed_id, parsed_idx) = parse_clarify_callback(&encoded).expect("must parse");
        assert_eq!(
            parsed_id, registry_key,
            "parsed id must equal the registry key"
        );
        assert_eq!(parsed_idx, 1);
    }

    #[test]
    fn parse_clarify_callback_rejects_wrong_prefix() {
        assert!(parse_clarify_callback("answer:foo:0").is_none());
    }

    #[test]
    fn parse_clarify_callback_rejects_missing_index() {
        assert!(parse_clarify_callback("clarify:foo").is_none());
    }

    #[test]
    fn parse_clarify_callback_rejects_non_numeric_index() {
        assert!(parse_clarify_callback("clarify:foo:bar").is_none());
    }

    #[test]
    fn clarify_dispatcher_trait_is_separate_from_message_dispatcher() {
        // Compile-time proof: ClarifyDispatcher has no supertrait relationship
        // with MessageDispatcher. Both are independently implementable.
        // This test exists to anchor the SOLID / OQ-3 requirement in the test
        // suite — if the traits are ever merged, remove this comment and update
        // the plan SUMMARY accordingly.
        fn _assert_independent<T: ClarifyDispatcher>() {}
        fn _assert_independent_msg<T: crate::send_message_tool::MessageDispatcher>() {}
    }

    // ---------------------------------------------------------------------------
    // Phase 48.2 Plan 10 (D-16/G-48.2-3): prerequisites() declares the real cause.
    // ---------------------------------------------------------------------------

    fn make_clarify_tool(platform: ironhermes_core::Platform) -> ClarifyTool {
        ClarifyTool::new(
            ironhermes_core::SessionKey::new(platform, "test-chat"),
            None,
            std::sync::Arc::new(crate::clarify_registry::PendingClarifyRegistry::new()),
            None,
            std::sync::Arc::new(ironhermes_core::Config::default()),
        )
    }

    /// Available shape (Local platform, no dispatcher needed): `prerequisites()`
    /// returns an empty vec and `unsatisfied_prerequisites()` returns nothing.
    #[test]
    fn clarify_prerequisites_empty_when_available() {
        let tool = make_clarify_tool(ironhermes_core::Platform::Local);
        assert!(tool.is_available(), "Local platform is always available");
        assert!(
            tool.prerequisites().is_empty(),
            "no prerequisite should be reported when available"
        );
        assert!(
            crate::registry::unsatisfied_prerequisites(&tool).is_empty(),
            "unsatisfied_prerequisites() must be empty when available"
        );
    }

    /// Unavailable shape (Web platform, no dispatcher): `prerequisites()` returns
    /// exactly one named, described runtime prerequisite, and
    /// `unsatisfied_prerequisites()` reports it too.
    #[test]
    fn clarify_prerequisites_one_runtime_entry_when_unavailable() {
        let tool = make_clarify_tool(ironhermes_core::Platform::Web);
        assert!(
            !tool.is_available(),
            "Web platform with no dispatcher must be unavailable (WR-04)"
        );
        let prereqs = tool.prerequisites();
        assert_eq!(prereqs.len(), 1, "exactly one prerequisite expected");
        assert_eq!(prereqs[0].kind, "runtime");
        assert_eq!(prereqs[0].name, "clarify_channel");
        assert!(
            !prereqs[0].description.is_empty(),
            "description must be non-empty — it is the sentence the card shows"
        );

        let missing = crate::registry::unsatisfied_prerequisites(&tool);
        assert_eq!(
            missing.len(),
            1,
            "unsatisfied_prerequisites() must report exactly one entry when unavailable"
        );
    }

    /// The property that actually closes the gap: whenever `is_available()` is
    /// false, `unsatisfied_prerequisites()` is non-empty. Uses the module-level
    /// `assert_unavailable_implies_nonempty_prereqs` helper shared with
    /// `send_message_tool`'s equivalent assertion, so a third tool can be
    /// dropped into the same check later.
    #[test]
    fn clarify_unavailable_implies_nonempty_unsatisfied_prerequisites() {
        super::assert_unavailable_implies_nonempty_prereqs(&make_clarify_tool(
            ironhermes_core::Platform::Web,
        ));
    }
}
