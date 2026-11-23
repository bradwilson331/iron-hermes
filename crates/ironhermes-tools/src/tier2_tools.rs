//! Tier 2 memory tools (ADR-0008, phase 58.2 D-12). Built by [`build_tier2_tools`] from an
//! `Arc<dyn Tier2Handle>`; later plans extend the name lists and the factory in this file only.

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use ironhermes_core::tier2::Tier2Handle;
use serde_json::json;

use crate::memory_tool::SharedMemoryManager;
use crate::registry::Tool;

/// Longest accepted `memory_search` query (T-58.2-01).
const MAX_QUERY_CHARS: usize = 500;
const DEFAULT_LIMIT: u64 = 10;
const MAX_LIMIT: u64 = 50;

pub const TIER2_TOOL_NAMES: &[&str] = &[
    "memory_search",
    "memory_explain",
    "memory_consolidate",
    "memory_forget",
    "memory_note",
];
/// What sandboxed code and delegate children get: neither `memory_consolidate` nor the
/// destructive `memory_forget` is ever among them.
pub const TIER2_READ_ONLY_TOOL_NAMES: &[&str] = &["memory_search", "memory_explain"];

/// 58.2 D-12: single source for the `memory` toolset members; later plans append their tool here
/// and to TIER2_TOOL_NAMES together.
pub const MEMORY_TOOLSET_MEMBERS: &[&str] = &[
    "memory",
    "memory_search",
    "memory_explain",
    "memory_consolidate",
    "memory_forget",
    "memory_note",
];

/// Which tools a caller gets: subagents are read-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier2ToolScope {
    Full,
    ReadOnly,
}

/// The Tier 2 handle attached to `manager`, if any. Registration is sync and runs at boot before
/// any turn holds the lock, so `try_lock` is enough; contention is loud, never silent.
pub fn tier2_handle_of(manager: &SharedMemoryManager) -> Option<Arc<dyn Tier2Handle>> {
    match manager.try_lock() {
        Ok(m) => m.tier2(),
        Err(_) => {
            tracing::error!(
                "Tier 2 tools not registered: memory manager locked during registration"
            );
            None
        }
    }
}

pub fn build_tier2_tools(
    handle: Arc<dyn Tier2Handle>,
    scope: Tier2ToolScope,
) -> Vec<Box<dyn Tool>> {
    let mut tools: Vec<Box<dyn Tool>> = vec![
        Box::new(MemorySearchTool {
            handle: handle.clone(),
        }),
        Box::new(MemoryExplainTool {
            handle: handle.clone(),
        }),
    ];
    // Consolidation spends model tokens and writes memory: full scope only (T-58.2-23).
    if scope == Tier2ToolScope::Full {
        tools.push(Box::new(MemoryConsolidateTool {
            handle: handle.clone(),
        }));
        // Destructive (T-58.2-28): never in a sandbox or a delegate child.
        tools.push(Box::new(MemoryForgetTool {
            handle: handle.clone(),
        }));
        // Writes memory: full scope only (D-31).
        tools.push(Box::new(MemoryNoteTool { handle }));
    }
    tools
}

pub struct MemorySearchTool {
    handle: Arc<dyn Tier2Handle>,
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn toolset(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Search long-term Tier 2 memory: claims consolidated from past conversations, MEMORY.md/USER.md \
         entries (including entries since moved out of them), and your daily notes. Returns claim ids \
         (for memory_explain), the claim text, its source, and whether the search was semantic or keyword."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What to look for, in natural language (max 500 characters)."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_LIMIT,
                        "default": DEFAULT_LIMIT,
                        "description": "Maximum number of results."
                    }
                },
                "required": ["query"]
            }),
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter 'query'"))?;
        if query.chars().count() > MAX_QUERY_CHARS {
            anyhow::bail!("'query' is too long (max {MAX_QUERY_CHARS} characters)");
        }
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT) as usize;
        let response = self.handle.search(query, limit).await?;
        Ok(serde_json::to_string(&response)?)
    }
}

pub struct MemoryExplainTool {
    handle: Arc<dyn Tier2Handle>,
}

#[async_trait]
impl Tool for MemoryExplainTool {
    fn name(&self) -> &str {
        "memory_explain"
    }

    fn toolset(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Explain a Tier 2 memory claim: its evidence (source, quote, timestamp), the older claims \
         it supersedes, and its confidence. Takes a claim id from memory_search."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "claim_id": {
                        "type": "string",
                        "description": "A claim id returned by memory_search, e.g. claim:0123456789ab."
                    }
                },
                "required": ["claim_id"]
            }),
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let id = args
            .get("claim_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter 'claim_id'"))?;
        // Validate before any store call: the id is model-written (T-58.2-13).
        if !ironhermes_core::tier2::is_valid_claim_id(id) {
            anyhow::bail!("'claim_id' must look like claim:0123456789ab");
        }
        match self.handle.explain(id).await? {
            Some(e) => Ok(serde_json::to_string(&e)?),
            None => Ok(json!({"error": "not_found"}).to_string()),
        }
    }
}

pub struct MemoryConsolidateTool {
    handle: Arc<dyn Tier2Handle>,
}

#[async_trait]
impl Tool for MemoryConsolidateTool {
    fn name(&self) -> &str {
        "memory_consolidate"
    }

    fn toolset(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Run Tier 2 memory consolidation now: extract cited claims from conversations since the \
         last run (also runs hourly in the gateway)."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            self.description(),
            json!({"type": "object", "properties": {}}),
        )
    }

    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        Ok(serde_json::to_string(&self.handle.consolidate().await?)?)
    }
}

/// Longest accepted `memory_forget` reason (stored only in pending.json).
const MAX_REASON_CHARS: usize = 200;

pub struct MemoryForgetTool {
    handle: Arc<dyn Tier2Handle>,
}

#[async_trait]
impl Tool for MemoryForgetTool {
    fn name(&self) -> &str {
        "memory_forget"
    }

    fn toolset(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Forget a Tier 2 memory claim: it is hidden from search, explain and recent lists immediately \
         and stays hidden. Staging deletes nothing: a human-run purge (`ironhermes memory purge` or \
         the Tier 2 tab) deletes a staged claim permanently, and you cannot run it. Action 'cancel' \
         restores the claim and works until that purge. Staging does not change MEMORY.md/USER.md. \
         Takes a claim id from memory_search."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "claim_id": {
                        "type": "string",
                        "description": "A claim id returned by memory_search, e.g. claim:0123456789ab."
                    },
                    "action": {
                        "type": "string",
                        "enum": ["stage", "cancel"],
                        "default": "stage",
                        "description": "stage: hide now (not deleted). cancel: undo a staged forget."
                    },
                    "reason": {
                        "type": "string",
                        "maxLength": MAX_REASON_CHARS,
                        "description": "Optional note on why (max 200 characters)."
                    }
                },
                "required": ["claim_id"]
            }),
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let id = args
            .get("claim_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter 'claim_id'"))?;
        // All three inputs are model-written: validate before any store call (T-58.2-31).
        if !ironhermes_core::tier2::is_valid_claim_id(id) {
            anyhow::bail!("'claim_id' must look like claim:0123456789ab");
        }
        let action = match args.get("action") {
            None | Some(serde_json::Value::Null) => "stage",
            Some(v) => v
                .as_str()
                .filter(|a| matches!(*a, "stage" | "cancel"))
                .ok_or_else(|| anyhow::anyhow!("'action' must be 'stage' or 'cancel'"))?,
        };
        let reason = match args.get("reason") {
            None | Some(serde_json::Value::Null) => None,
            Some(v) => Some(
                v.as_str()
                    .filter(|r| r.chars().count() <= MAX_REASON_CHARS)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "'reason' must be a string of at most {MAX_REASON_CHARS} characters"
                        )
                    })?,
            ),
        };
        let outcome = if action == "cancel" {
            self.handle.unstage_forget(id).await?
        } else {
            self.handle.stage_forget(id, reason).await?
        };
        Ok(serde_json::to_string(&outcome)?)
    }
}

pub struct MemoryNoteTool {
    handle: Arc<dyn Tier2Handle>,
}

#[async_trait]
impl Tool for MemoryNoteTool {
    fn name(&self) -> &str {
        "memory_note"
    }

    fn toolset(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Write a note to today's long-term daily notes for day-to-day detail that should not take \
         space in MEMORY.md or USER.md. Notes are never shown in the prompt; find them later with \
         memory_search."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "maxLength": 1000,
                        "description": "One day-to-day detail to remember, max 1000 characters"
                    }
                },
                "required": ["content"]
            }),
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter 'content'"))?;
        let out = self.handle.write_note(content).await?;
        Ok(json!({
            "claim_id": out.claim_id,
            "source": out.source,
            "note": "saved to today's daily notes; not shown in the prompt; recall with memory_search",
        })
        .to_string())
    }
}

/// Shared test doubles (58.2-03). Later plans that add `Tier2Handle` methods extend only this
/// stub; the trait has no defaulted methods, so a real impl never silently inherits a no-op.
#[cfg(test)]
pub(crate) struct StubTier2Handle;

#[cfg(test)]
#[async_trait]
impl Tier2Handle for StubTier2Handle {
    async fn search(
        &self,
        _q: &str,
        _l: usize,
    ) -> anyhow::Result<ironhermes_core::tier2::Tier2SearchResponse> {
        Ok(ironhermes_core::tier2::Tier2SearchResponse {
            results: vec![],
            mode: ironhermes_core::tier2::Tier2SearchMode::Keyword,
            note: None,
        })
    }
    async fn sync_tier1(
        &self,
        _e: &ironhermes_core::MemoryEntries,
    ) -> anyhow::Result<ironhermes_core::tier2::Tier1SyncReport> {
        Ok(ironhermes_core::tier2::Tier1SyncReport::default())
    }
    async fn explain(
        &self,
        _id: &str,
    ) -> anyhow::Result<Option<ironhermes_core::tier2::Tier2Explanation>> {
        Ok(None)
    }
    async fn list_recent(
        &self,
        _l: usize,
    ) -> anyhow::Result<Vec<ironhermes_core::tier2::Tier2Hit>> {
        Ok(vec![])
    }
    async fn consolidate(&self) -> anyhow::Result<ironhermes_core::tier2::ConsolidationReport> {
        Ok(Default::default())
    }
    async fn run_dreams(&self) -> anyhow::Result<ironhermes_core::tier2::DreamsReport> {
        Ok(Default::default())
    }
    async fn synthesis_snapshot(&self, _s: Option<&str>) -> Option<String> {
        None
    }
    async fn stage_forget(
        &self,
        id: &str,
        _r: Option<&str>,
    ) -> anyhow::Result<ironhermes_core::tier2::ForgetOutcome> {
        Ok(ironhermes_core::tier2::ForgetOutcome {
            claim_id: id.into(),
            status: "staged".into(),
            note: String::new(),
        })
    }
    async fn unstage_forget(
        &self,
        id: &str,
    ) -> anyhow::Result<ironhermes_core::tier2::ForgetOutcome> {
        Ok(ironhermes_core::tier2::ForgetOutcome {
            claim_id: id.into(),
            status: "cancelled".into(),
            note: String::new(),
        })
    }
    async fn list_staged(&self) -> anyhow::Result<Vec<ironhermes_core::tier2::Tier2Hit>> {
        Ok(vec![])
    }
}

/// Mock `MemoryManagerHandle` whose `tier2()` is configurable.
#[cfg(test)]
pub(crate) struct StubMemoryManager(pub Option<Arc<dyn Tier2Handle>>);

#[cfg(test)]
#[async_trait]
impl crate::memory_manager_handle::MemoryManagerHandle for StubMemoryManager {
    async fn handle_tool_call(
        &self,
        _name: &str,
        _args: serde_json::Value,
    ) -> ironhermes_core::memory_store::MemoryResult {
        Ok("{}".to_string())
    }
    fn tier2(&self) -> Option<Arc<dyn Tier2Handle>> {
        self.0.clone()
    }
}

#[cfg(test)]
pub(crate) fn stub_manager(with_tier2: bool) -> crate::memory_tool::SharedMemoryManager {
    let h: Option<Arc<dyn Tier2Handle>> = with_tier2.then(|| Arc::new(StubTier2Handle) as _);
    Arc::new(tokio::sync::Mutex::new(StubMemoryManager(h)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::MemoryEntries;
    use ironhermes_core::tier2::{
        Tier1SyncReport, Tier2Explanation, Tier2Hit, Tier2SearchMode, Tier2SearchResponse,
    };
    use std::sync::Mutex;

    /// Records the (query, limit) it was asked for.
    struct Spy(
        Mutex<Vec<(String, usize)>>,
        Mutex<Vec<String>>,
        Mutex<Vec<String>>,
    );

    #[async_trait]
    impl Tier2Handle for Spy {
        async fn search(&self, q: &str, l: usize) -> anyhow::Result<Tier2SearchResponse> {
            self.0.lock().unwrap().push((q.to_string(), l));
            Ok(Tier2SearchResponse {
                results: vec![],
                mode: Tier2SearchMode::Keyword,
                note: Some("n".into()),
            })
        }
        async fn sync_tier1(&self, _e: &MemoryEntries) -> anyhow::Result<Tier1SyncReport> {
            Ok(Tier1SyncReport::default())
        }
        async fn explain(&self, id: &str) -> anyhow::Result<Option<Tier2Explanation>> {
            self.1.lock().unwrap().push(id.to_string());
            Ok(None)
        }
        async fn list_recent(&self, _l: usize) -> anyhow::Result<Vec<Tier2Hit>> {
            Ok(vec![])
        }
        async fn consolidate(&self) -> anyhow::Result<ironhermes_core::tier2::ConsolidationReport> {
            Ok(Default::default())
        }
        async fn run_dreams(&self) -> anyhow::Result<ironhermes_core::tier2::DreamsReport> {
            Ok(Default::default())
        }
        async fn synthesis_snapshot(&self, _s: Option<&str>) -> Option<String> {
            None
        }
        async fn stage_forget(
            &self,
            id: &str,
            _r: Option<&str>,
        ) -> anyhow::Result<ironhermes_core::tier2::ForgetOutcome> {
            self.2.lock().unwrap().push(format!("stage:{id}"));
            Ok(ironhermes_core::tier2::ForgetOutcome {
                claim_id: id.into(),
                status: "staged".into(),
                note: String::new(),
            })
        }
        async fn unstage_forget(
            &self,
            id: &str,
        ) -> anyhow::Result<ironhermes_core::tier2::ForgetOutcome> {
            self.2.lock().unwrap().push(format!("cancel:{id}"));
            Ok(ironhermes_core::tier2::ForgetOutcome {
                claim_id: id.into(),
                status: "cancelled".into(),
                note: String::new(),
            })
        }
        async fn list_staged(&self) -> anyhow::Result<Vec<Tier2Hit>> {
            Ok(vec![])
        }
    }

    fn tool() -> (MemorySearchTool, Arc<Spy>) {
        let spy = Arc::new(Spy(
            Mutex::new(vec![]),
            Mutex::new(vec![]),
            Mutex::new(vec![]),
        ));
        (
            MemorySearchTool {
                handle: spy.clone(),
            },
            spy,
        )
    }

    #[tokio::test]
    async fn rejects_empty_and_oversized_queries() {
        let (t, spy) = tool();
        assert!(t.execute(json!({})).await.is_err());
        assert!(t.execute(json!({"query": "   "})).await.is_err());
        assert!(t.execute(json!({"query": "x".repeat(501)})).await.is_err());
        assert!(t.execute(json!({"query": "x".repeat(500)})).await.is_ok());
        assert_eq!(
            spy.0.lock().unwrap().len(),
            1,
            "rejected queries never reach the handle"
        );
    }

    #[tokio::test]
    async fn explain_rejects_malformed_id() {
        let spy = Arc::new(Spy(
            Mutex::new(vec![]),
            Mutex::new(vec![]),
            Mutex::new(vec![]),
        ));
        let t = MemoryExplainTool {
            handle: spy.clone(),
        };
        for bad in ["claim:XYZ", "abc123", "claim:0123456789ab'; DROP"] {
            assert!(t.execute(json!({"claim_id": bad})).await.is_err(), "{bad}");
        }
        assert!(t.execute(json!({})).await.is_err());
        assert!(
            spy.1.lock().unwrap().is_empty(),
            "malformed ids never reach the handle"
        );
        let out = t
            .execute(json!({"claim_id": "claim:0123456789ab"}))
            .await
            .unwrap();
        assert_eq!(out, r#"{"error":"not_found"}"#);
        assert_eq!(spy.1.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn clamps_limit() {
        let (t, spy) = tool();
        t.execute(json!({"query": "a"})).await.unwrap();
        t.execute(json!({"query": "a", "limit": 0})).await.unwrap();
        t.execute(json!({"query": "a", "limit": 9999}))
            .await
            .unwrap();
        let limits: Vec<usize> = spy.0.lock().unwrap().iter().map(|(_, l)| *l).collect();
        assert_eq!(limits, vec![10, 1, 50]);
    }

    #[test]
    fn scopes_expose_the_documented_names() {
        let (_, spy) = tool();
        for (scope, want) in [
            (Tier2ToolScope::Full, TIER2_TOOL_NAMES),
            (Tier2ToolScope::ReadOnly, TIER2_READ_ONLY_TOOL_NAMES),
        ] {
            let names: Vec<String> = build_tier2_tools(spy.clone(), scope)
                .iter()
                .map(|t| t.name().to_string())
                .collect();
            assert_eq!(names, want);
        }
    }

    #[test]
    fn memory_consolidate_full_scope_only() {
        let names = |scope| -> Vec<String> {
            build_tier2_tools(Arc::new(StubTier2Handle), scope)
                .iter()
                .map(|t| t.name().to_string())
                .collect()
        };
        assert!(names(Tier2ToolScope::Full).contains(&"memory_consolidate".to_string()));
        assert!(!names(Tier2ToolScope::ReadOnly).contains(&"memory_consolidate".to_string()));
        assert!(TIER2_TOOL_NAMES.contains(&"memory_consolidate"));
        assert!(!TIER2_READ_ONLY_TOOL_NAMES.contains(&"memory_consolidate"));
        assert!(MEMORY_TOOLSET_MEMBERS.contains(&"memory_consolidate"));
    }

    #[test]
    fn memory_toolset_members_match_tier2_tool_names() {
        let mut want = vec!["memory"];
        want.extend(TIER2_TOOL_NAMES);
        assert_eq!(MEMORY_TOOLSET_MEMBERS, want.as_slice());
        for n in TIER2_READ_ONLY_TOOL_NAMES {
            assert!(TIER2_TOOL_NAMES.contains(n), "{n} not in TIER2_TOOL_NAMES");
        }
        let names = |scope| -> Vec<String> {
            build_tier2_tools(Arc::new(StubTier2Handle), scope)
                .iter()
                .map(|t| t.name().to_string())
                .collect()
        };
        assert_eq!(names(Tier2ToolScope::Full), TIER2_TOOL_NAMES);
        assert_eq!(names(Tier2ToolScope::ReadOnly), TIER2_READ_ONLY_TOOL_NAMES);
    }

    #[test]
    fn memory_forget_full_scope_only() {
        let names = |scope| -> Vec<String> {
            build_tier2_tools(Arc::new(StubTier2Handle), scope)
                .iter()
                .map(|t| t.name().to_string())
                .collect()
        };
        assert!(names(Tier2ToolScope::Full).contains(&"memory_forget".to_string()));
        assert!(!names(Tier2ToolScope::ReadOnly).contains(&"memory_forget".to_string()));
        assert!(TIER2_TOOL_NAMES.contains(&"memory_forget"));
        assert!(!TIER2_READ_ONLY_TOOL_NAMES.contains(&"memory_forget"));
        assert!(MEMORY_TOOLSET_MEMBERS.contains(&"memory_forget"));

        let t = MemoryForgetTool {
            handle: Arc::new(StubTier2Handle),
        };
        let schema = serde_json::to_value(t.schema()).unwrap().to_string();
        assert!(schema.contains("claim_id") && schema.contains("\"required\":[\"claim_id\"]"));
        assert!(
            schema.contains("\"enum\":[\"stage\",\"cancel\"]"),
            "{schema}"
        );
        assert!(schema.contains("\"default\":\"stage\"") && schema.contains("reason"));
    }

    #[tokio::test]
    async fn memory_note_full_scope_only() {
        let names = |scope| -> Vec<String> {
            build_tier2_tools(Arc::new(StubTier2Handle), scope)
                .iter()
                .map(|t| t.name().to_string())
                .collect()
        };
        assert!(names(Tier2ToolScope::Full).contains(&"memory_note".to_string()));
        assert!(!names(Tier2ToolScope::ReadOnly).contains(&"memory_note".to_string()));
        assert!(TIER2_TOOL_NAMES.contains(&"memory_note"));
        assert!(!TIER2_READ_ONLY_TOOL_NAMES.contains(&"memory_note"));
        assert!(MEMORY_TOOLSET_MEMBERS.contains(&"memory_note"));

        let spy = Arc::new(Spy(
            Mutex::new(vec![]),
            Mutex::new(vec![]),
            Mutex::new(vec![]),
        ));
        let t = MemoryNoteTool { handle: spy };
        let schema = serde_json::to_value(t.schema()).unwrap().to_string();
        assert!(schema.contains("\"required\":[\"content\"]"), "{schema}");
        // The Spy keeps the default `write_note`, which errors: a missing `content` must fail
        // with its own message, proving validation runs before the handle is called.
        for bad in [json!({}), json!({"content": 7})] {
            let err = t.execute(bad).await.unwrap_err().to_string();
            assert!(
                err.contains("Missing required parameter 'content'"),
                "{err}"
            );
        }
        let err = t.execute(json!({"content": "x"})).await.unwrap_err();
        assert!(err.to_string().contains("not supported"), "{err}");

        let (search, _) = tool();
        assert!(search.description().contains("daily notes"));
    }

    #[tokio::test]
    async fn memory_forget_validates_args() {
        let spy = Arc::new(Spy(
            Mutex::new(vec![]),
            Mutex::new(vec![]),
            Mutex::new(vec![]),
        ));
        let t = MemoryForgetTool {
            handle: spy.clone(),
        };
        let id = "claim:0123456789ab";
        for bad in [
            json!({}),
            json!({"claim_id": "claim:XYZ"}),
            json!({"claim_id": "abc123"}),
            json!({"claim_id": "claim:0123456789ab'; DROP"}),
            json!({"claim_id": id, "action": "purge"}),
            json!({"claim_id": id, "action": 7}),
            json!({"claim_id": id, "reason": "x".repeat(201)}),
        ] {
            assert!(t.execute(bad.clone()).await.is_err(), "{bad}");
        }
        assert!(
            spy.2.lock().unwrap().is_empty(),
            "invalid calls never reach the handle"
        );

        let staged = t.execute(json!({"claim_id": id})).await.unwrap();
        assert!(staged.contains("\"status\":\"staged\""), "{staged}");
        t.execute(json!({"claim_id": id, "action": "stage", "reason": "x".repeat(200)}))
            .await
            .unwrap();
        let cancelled = t
            .execute(json!({"claim_id": id, "action": "cancel"}))
            .await
            .unwrap();
        assert!(
            cancelled.contains("\"status\":\"cancelled\""),
            "{cancelled}"
        );
        assert_eq!(
            *spy.2.lock().unwrap(),
            vec![
                format!("stage:{id}"),
                format!("stage:{id}"),
                format!("cancel:{id}")
            ]
        );
    }

    #[test]
    fn memory_forget_description_names_human_purge() {
        let t = MemoryForgetTool {
            handle: Arc::new(StubTier2Handle),
        };
        let d = t.description();
        assert!(!d.contains("no purge"), "{d}");
        assert!(d.contains("human-run purge"), "{d}");
        assert!(
            d.contains("ironhermes memory purge") && d.contains("Tier 2 tab"),
            "{d}"
        );
        assert!(
            d.contains("permanently") && d.contains("'cancel'") && d.contains("until that purge"),
            "{d}"
        );
    }

    fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                rs_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    /// D-28: no agent-reachable code names the purge. Walks every workspace member's `src/` (members
    /// read from the root Cargo.toml, so new files and crates are covered without editing this
    /// test) and fails when a file outside the allowlist names a purge identifier, when an
    /// allowlisted file registers an agent tool, or when a tool name contains the word.
    ///
    /// shortcut: text scan, not a call graph; a wrapper added inside an allowlisted file is caught
    /// by review, not this test; upgrade if the allowlist grows.
    #[test]
    fn no_agent_tool_can_purge() {
        use std::path::{Path, PathBuf};
        let root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf();
        let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        let block = manifest
            .split("\nmembers = [")
            .nth(1)
            .expect("root Cargo.toml has a `members = [` list")
            .split(']')
            .next()
            .unwrap();
        let members: Vec<&str> = block
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with('"'))
            .map(|l| l.trim_end_matches(',').trim_matches('"'))
            .collect();
        assert!(members.len() >= 20, "only {} members parsed", members.len());
        let mut files = Vec::new();
        for m in &members {
            let src = root.join(m).join("src");
            assert!(
                src.is_dir(),
                "member {m} has no src/ (glob or moved member?)"
            );
            rs_files(&src, &mut files);
        }
        assert!(files.len() >= 500, "only {} files scanned", files.len());
        let rel = |p: &Path| {
            p.strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        };
        let scanned: Vec<String> = files.iter().map(|p| rel(p)).collect();
        for canary in [
            "crates/ironhermes-tools/src/registry.rs",
            "crates/ironhermes-tools/src/execute_code.rs",
            "crates/ironhermes-tools/src/delegate_task.rs",
            "crates/ironhermes-agent/src/app_runtime_factory.rs",
            "crates/iron_hermes_ui/src/server/api.rs",
        ] {
            assert!(scanned.iter().any(|f| f == canary), "walk missed {canary}");
        }

        // Halves joined at runtime so this file never contains a needle itself.
        let needles = [
            ["run_", "purge"].concat(),
            ["purge", "_preview"].concat(),
            ["purge_", "tier2"].concat(),
            ["preview_", "tier2"].concat(),
            ["purge", "_now"].concat(),
        ];
        let allow = [
            "crates/ironhermes-core/src/tier2.rs",
            "crates/ironhermes-tier2/src/",
            "crates/ironhermes-agent/src/memory/tier2_maintenance.rs",
            "crates/ironhermes-cli/src/memory_cmd.rs",
            "crates/iron_hermes_ui/src/server/tier2_api.rs",
            "crates/iron_hermes_ui/src/components/hermes_app/screens/memory_tier2.rs",
        ];
        let markers = [
            "Tool for ",
            ".register(",
            "register_dynamic(",
            "register_intercepted",
            "build_tier2_tools(",
            "Tier2ToolScope",
            "register_memory_tool",
        ];
        let mut positive_control = [false; 2];
        for (f, path) in scanned.iter().zip(&files) {
            let text = String::from_utf8_lossy(&std::fs::read(path).unwrap()).into_owned();
            let allowed = allow.iter().any(|a| f.starts_with(a));
            for n in &needles {
                if text.contains(n.as_str()) {
                    assert!(
                        allowed,
                        "{f} names `{n}` but is not an allowlisted human entry point"
                    );
                }
            }
            if allowed {
                for m in markers {
                    assert!(
                        !text.contains(m),
                        "allowlisted {f} contains tool-registration marker `{m}`: it could be an agent tool"
                    );
                }
            }
            if f == "crates/ironhermes-core/src/tier2.rs" {
                positive_control = [
                    text.contains(needles[0].as_str()),
                    text.contains(needles[1].as_str()),
                ];
            }
        }
        assert_eq!(positive_control, [true, true], "walk read the wrong root");
        for name in TIER2_TOOL_NAMES.iter().chain(MEMORY_TOOLSET_MEMBERS) {
            assert!(!name.contains("purge"), "tool name {name} names the purge");
        }
    }

    #[test]
    fn tier2_handle_of_returns_handle_when_enabled() {
        assert!(tier2_handle_of(&stub_manager(true)).is_some());
        assert!(tier2_handle_of(&stub_manager(false)).is_none());
    }
}
