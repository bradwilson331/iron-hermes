use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use ironhermes_cron::JobStore;

use crate::memory_tool::SharedMemoryManager;

/// D-09 / D-25 (Phase 25): per-tool prerequisite descriptor for setup-wizard discovery.
/// Plain-String type per cross-crate convention (Phase 22.4.2.2 → 23 D-12 → 24 D-17 → 25 D-25).
#[derive(Debug, Clone)]
pub struct Prerequisite {
    /// "env_var" | "config_field" (string union per D-25; downstream matches on kind at call site).
    pub kind: String,
    /// e.g. "FIRECRAWL_API_KEY" or "search.brave_api_key" (dotted-path config key).
    pub name: String,
    /// Human-readable description shown by the setup wizard (D-18).
    pub description: String,
    /// true = blocks is_available() when missing; false = optional / advisory only.
    pub required: bool,
    /// D-09 (Phase 41.3): any-of group id. Entries sharing a group id are satisfied when
    /// **any one** member is present. `None` means the entry stands alone and `required`
    /// governs it as before.
    pub group: Option<String>,
}

impl Prerequisite {
    /// D-09 (Phase 41.3): ungrouped env-var prerequisite constructor, so call sites do not
    /// hand-roll the literal (and its `group: None`) themselves.
    pub fn env_var(
        name: impl Into<String>,
        description: impl Into<String>,
        required: bool,
    ) -> Self {
        Self {
            kind: "env_var".to_string(),
            name: name.into(),
            description: description.into(),
            required,
            group: None,
        }
    }

    /// D-09 (Phase 41.3): any-of group member constructor for multi-provider tools. `required`
    /// is `false` by construction — an individual group member is never independently
    /// required, or the AND-over-required default would demand every provider's key. The
    /// group as a whole is satisfied when any one member's env var is present (see the
    /// group-aware default `Tool::is_available()`).
    pub fn grouped_env_var(
        name: impl Into<String>,
        description: impl Into<String>,
        group: impl Into<String>,
    ) -> Self {
        Self {
            kind: "env_var".to_string(),
            name: name.into(),
            description: description.into(),
            required: false,
            group: Some(group.into()),
        }
    }

    /// Phase 48.2 Plan 10 (D-16/G-48.2-3): a runtime-condition prerequisite constructor.
    ///
    /// `env_var` and `config_field` are STATIC declarations whose satisfaction is looked
    /// up (an env var either is or is not set; a config field either is or is not
    /// present) — a tool can safely declare them unconditionally and let
    /// `prerequisite_satisfied` decide. A `runtime` prerequisite is the opposite shape:
    /// it is a declaration that a condition the PROCESS ITSELF controls (a dispatcher
    /// wired for this session, a platform capability, …) is CURRENTLY unmet, and
    /// `prerequisite_satisfied`'s `"runtime" => false` arm answers it unconditionally
    /// `false` with no lookup at all (see that arm's doc comment). This inverts who is
    /// responsible for conditionality: because the kind itself never resolves to
    /// satisfied, a tool must emit a `runtime` prerequisite ONLY when the condition it
    /// names does not currently hold, and must return an empty `prerequisites()` list
    /// when it does — never emit one unconditionally, or the default `is_available()`
    /// (which ANDs every ungrouped `required: true` prerequisite) gates the tool closed
    /// permanently, even when the tool overrides `is_available()` itself and would
    /// otherwise report available (because `unsatisfied_prerequisites()` walks
    /// `prerequisites()` independently of whatever `is_available()` returns). Always
    /// `required: true` and `group: None` — a runtime cause is not optional and is
    /// never a member of an any-of group.
    pub fn runtime(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            kind: "runtime".to_string(),
            name: name.into(),
            description: description.into(),
            required: true,
            group: None,
        }
    }
}

/// D-09 (Phase 41.3): shared per-prerequisite satisfaction check, independent of a
/// prerequisite's `required`/`group` fields. Used by both the group-aware default
/// `Tool::is_available()` and `satisfied_group_members()` so there is exactly one
/// definition of "is this prerequisite met".
///
/// `tool_name`/`credentials` come from the resolving `Tool` (`self.name()` /
/// `self.credentials()`, D-19) and are threaded in only for the `config_field` arm
/// (D-18): a `config_field` prerequisite gates on
/// `credentials.config_field_present(&p.name)`, and a tool with no snapshot gates
/// **closed** (`unwrap_or(false)`) — the D-18 reversal of the pre-Plan-10
/// "config_field is non-blocking" default. `satisfied_group_members()` has no
/// `Tool` to draw either from and passes `None` for both; no production
/// `config_field` prerequisite is ever a group member today
/// (`Prerequisite::grouped_env_var` always sets `kind: "env_var"`), so this is a
/// no-op for every existing caller. `env_var` and unknown-kind semantics are
/// unchanged from pre-D-18 (env_var checks presence; unknown kinds are
/// non-blocking by design per D-25).
fn prerequisite_satisfied(
    tool_name: Option<&str>,
    p: &Prerequisite,
    credentials: Option<&crate::credentials::ToolCredentials>,
) -> bool {
    match p.kind.as_str() {
        // Phase 41.3 Plan 09 (D-19 fix): when a resolved snapshot is available,
        // defer to it — `ToolCredentials::has_credential` answers env -> config
        // -> vault, not env alone, so a key configured only in
        // `tools.credentials` or only in the vault is correctly counted as
        // satisfied here too. `env_only()`'s snapshot (every existing call
        // site that never resolves a real one) answers identically to the old
        // bare `std::env::var` check, so this is a no-op for those callers.
        // Falling back to the bare check when `credentials` is `None` (e.g.
        // `satisfied_group_members`'s public two-arg API) preserves this
        // function's pre-existing behavior for every caller that has no
        // snapshot to offer.
        "env_var" => credentials
            .map(|c| c.has_credential(&p.name))
            .unwrap_or_else(|| std::env::var(&p.name).is_ok()),
        "config_field" => {
            let satisfied = credentials
                .map(|c| c.config_field_present(&p.name))
                .unwrap_or(false);
            if !satisfied {
                // D-18: a tool vanishing from the model's schema is otherwise
                // invisible — loud, not silent. Names the tool and the
                // prerequisite's dotted path; never a value.
                tracing::warn!(
                    tool = tool_name.unwrap_or("<unknown>"),
                    prereq_name = %p.name,
                    "config_field prerequisite gated a tool closed — no credential snapshot satisfies it"
                );
            }
            satisfied
        }
        // Phase 48.2 Plan 10: a `runtime` prerequisite is emitted by a tool ONLY when
        // the condition it names is already unmet (see `Prerequisite::runtime`'s doc
        // comment) — there is nothing to look up, so this arm always answers `false`.
        // This is NOT a repurposing of the `_ => true` fallback below: that fallback
        // exists so a genuinely UNKNOWN kind stays non-blocking (D-25), whereas
        // `"runtime"` is a KNOWN kind whose entire contract is "unconditionally
        // unsatisfied when present". Placed before the fallback so it is matched
        // explicitly rather than falling through.
        "runtime" => false,
        _ => true, // unknown kinds are non-blocking by design (D-25)
    }
}

/// D-09 (Phase 41.3): the prerequisites in `prereqs` sharing `group`, in declaration
/// order. Shared by the group-aware default `Tool::is_available()` and by consumers
/// (setup wizard, `doctor` — Plan 09) that need to enumerate a group's members rather
/// than re-implementing the partitioning.
pub fn group_members<'a>(prereqs: &'a [Prerequisite], group: &str) -> Vec<&'a Prerequisite> {
    prereqs
        .iter()
        .filter(|p| p.group.as_deref() == Some(group))
        .collect()
}

/// D-09 (Phase 41.3): count of `group`'s members currently satisfied — the "N" in
/// `doctor`'s "N/M providers configured" rendering (Plan 09). No `Tool` handle is
/// available here, so `prerequisite_satisfied` is called with `tool_name: None`,
/// `credentials: None` — a no-op for the `config_field` arm today, since no
/// production `config_field` prerequisite is ever a group member (see
/// `prerequisite_satisfied`'s doc comment).
pub fn satisfied_group_members(prereqs: &[Prerequisite], group: &str) -> usize {
    satisfied_group_members_with_credentials(prereqs, group, None)
}

/// Phase 41.3 Plan 09 (D-19): credential-snapshot-aware sibling of
/// [`satisfied_group_members`] — for a caller (`doctor`, Plan 09) that DOES
/// have a resolved [`crate::credentials::ToolCredentials`] snapshot on hand
/// and needs its N-of-M count to agree with what the runtime's own
/// `is_available()` would answer for the same credential state, not just
/// with live process env. `credentials: None` reproduces
/// `satisfied_group_members`'s exact behavior (see its doc comment on why
/// this is safe for every existing caller).
pub fn satisfied_group_members_with_credentials(
    prereqs: &[Prerequisite],
    group: &str,
    credentials: Option<&crate::credentials::ToolCredentials>,
) -> usize {
    group_members(prereqs, group)
        .into_iter()
        .filter(|p| prerequisite_satisfied(None, p, credentials))
        .count()
}

/// Phase 48.2 Plan 01 (D-16): a group-aware INVERSE of the trait-default
/// `Tool::is_available()` walk — returns the prerequisites that are currently
/// UNSATISFIED for `tool`, so the Tools page can name them on an amber card.
///
/// Per-entry satisfaction is delegated to the same private
/// [`prerequisite_satisfied`] helper `is_available()` uses, passing
/// `tool.name()` and `tool.credentials()` exactly as the trait default does —
/// there is exactly one definition of "is this prerequisite met" in this
/// crate. The returned set is: every ungrouped `required: true` entry that is
/// unsatisfied, plus — for each distinct `group` id — ALL of that group's
/// members, but only when NO member of the group is satisfied (an any-of
/// group failing entirely is reported as its full member list, mirroring
/// `is_available()`'s any-of semantics; a group with at least one satisfied
/// member is not a missing prerequisite at all).
pub fn unsatisfied_prerequisites(tool: &dyn Tool) -> Vec<Prerequisite> {
    let prereqs = tool.prerequisites();
    let tool_name = Some(tool.name());
    let credentials = tool.credentials();

    let mut missing: Vec<Prerequisite> = prereqs
        .iter()
        .filter(|p| p.group.is_none() && p.required)
        .filter(|p| !prerequisite_satisfied(tool_name, p, credentials))
        .cloned()
        .collect();

    let mut groups: Vec<&str> = prereqs.iter().filter_map(|p| p.group.as_deref()).collect();
    groups.sort_unstable();
    groups.dedup();

    for group in groups {
        let members = group_members(&prereqs, group);
        let any_satisfied = members
            .iter()
            .any(|p| prerequisite_satisfied(tool_name, p, credentials));
        if !any_satisfied {
            missing.extend(members.into_iter().cloned());
        }
    }

    missing
}

/// Toolset sentinel reported by dynamically-discovered MCP tools (`McpTool::toolset()`).
///
/// MCP tools are NOT part of the built-in toolset taxonomy (`ALL_TOOLSETS`): they are
/// discovered at runtime from connected MCP servers, so no config entry exists for the
/// `"mcp"` toolset. `get_definitions()` exempts this toolset from the toolset-enabled
/// filter — otherwise `is_toolset_enabled("mcp")` returns `false` (absent from the config
/// map) and every discovered MCP tool is silently hidden from the LLM (fail-closed).
/// MCP availability is instead gated by server connection + `McpMutationGuardrail` +
/// the per-tool `disabled` list. Single source of truth shared with `ironhermes-mcp`.
pub const MCP_TOOLSET: &str = "mcp";

/// Toolset reported by the kanban worker-protocol tools (`kanban_show`, `kanban_complete`,
/// `kanban_block`, …).
///
/// Like [`MCP_TOOLSET`], the `"kanban"` toolset is NOT part of the built-in toolset taxonomy
/// (`ALL_TOOLSETS`) and is absent from every user config — it is an operational, process-scoped
/// toolset that only exists inside a kanban worker (env `IRONHERMES_KANBAN_TASK` set) or an
/// orchestrator that explicitly enabled it. These tools are registered on-demand and self-gate
/// via each tool's `is_available()` (worker env OR explicit-enable). Without a `get_definitions()`
/// exemption, `is_toolset_enabled("kanban")` returns `false` (absent from the config map) and the
/// layer-1 filter silently hides `kanban_complete`/`kanban_block` from the worker's LLM — leaving
/// the worker with no way to terminate its task, so it improvises (`kanban_show` as a shell
/// command) and the task never completes. Visibility stays correctly scoped by `is_available()`.
pub const KANBAN_TOOLSET: &str = "kanban";

/// Phase 48.2 Plan 11 (G-48.2-6 slice a): the single stable identifier a
/// `Tool::runtime_dependency()` override returns to name the gateway
/// process — never spelled as a string literal at any call site. The Tools
/// page's `iron_hermes_ui::server::tools_config_api` crate keeps a portable
/// byte-identical duplicate of this constant (that crate is native-only, so
/// the wasm client cannot name this crate's types directly), pinned by a
/// native-only drift test.
pub const GATEWAY_RUNTIME_DEPENDENCY: &str = "gateway";

/// D-04/D-05/D-15 (Phase 41.3): trait-default wall-clock execution budget
/// (seconds), returned by `Tool::timeout_secs()` when a tool does not
/// override it, and used as the operator-config floor (D-06 level 4). 60s is
/// 2x `WebConfig.timeout_secs`'s 30s HTTP-leg default, so a wedged web call
/// surfaces inside a minute.
pub const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 60;

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn toolset(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> ToolSchema;

    /// D-09 (Phase 41.3): group-aware default. Partitions `prerequisites()` by
    /// `group`: every ungrouped entry with `required: true` must be satisfied
    /// (AND — unchanged from pre-D-09 behavior), and for each distinct group
    /// id, **at least one** member must be satisfied (OR), regardless of that
    /// member's own `required` value (grouped entries are `required: false` by
    /// construction — see `Prerequisite::grouped_env_var`). The tool is
    /// available iff the ungrouped AND holds and every group's OR holds. A
    /// tool with no prerequisites is available, as before. Tools may still
    /// override this for custom logic, but MUST also implement
    /// `prerequisites()` for setup-wizard discovery (D-09). This any-of check
    /// lives once, here — multi-provider tools (Plan 07/08) must not
    /// re-implement it themselves.
    fn is_available(&self) -> bool {
        let prereqs = self.prerequisites();
        let tool_name = Some(self.name());
        let credentials = self.credentials();

        let ungrouped_ok = prereqs
            .iter()
            .filter(|p| p.group.is_none() && p.required)
            .all(|p| prerequisite_satisfied(tool_name, p, credentials));

        let mut groups: Vec<&str> = prereqs.iter().filter_map(|p| p.group.as_deref()).collect();
        groups.sort_unstable();
        groups.dedup();

        let groups_ok = groups.iter().all(|group| {
            group_members(&prereqs, group)
                .into_iter()
                .any(|p| prerequisite_satisfied(tool_name, p, credentials))
        });

        ungrouped_ok && groups_ok
    }

    /// Per-tool prerequisite list for setup-wizard discovery (D-09 / Phase 25).
    /// Default returns empty Vec (most tools have no external prerequisites).
    fn prerequisites(&self) -> Vec<Prerequisite> {
        vec![]
    }

    /// D-19 (Phase 41.3): the pre-resolved credential snapshot this tool was
    /// registered with, if any. Default `None` — all existing `impl Tool` blocks
    /// inherit this with zero edits, the same zero-edit guarantee `on_session_end`
    /// (above) and Plan 06's `group` field were built on.
    ///
    /// This is the D-19 seam: `is_available()` is sync and
    /// `SecretStore::get_secret` is async, so a tool that needs a
    /// credential-backed availability answer carries a pre-resolved
    /// `ToolCredentials` snapshot and hands it back here — the default
    /// `is_available()`'s `config_field` arm (below) reads it via
    /// `self.credentials()` rather than ever reaching for the vault directly.
    fn credentials(&self) -> Option<&crate::credentials::ToolCredentials> {
        None
    }

    /// Phase 48.2 Plan 01 (D-20, `<assumption_delta_decision>`): a UI-facing
    /// catalog grouping override, entirely separate from [`Tool::toolset`].
    ///
    /// `None` (the default) means the display group equals `toolset()` — no
    /// behavior change for any existing tool. `Some(g)` overrides only the
    /// Tools page's grouping — it is NEVER consulted by [`ToolRegistry::get_definitions`]
    /// and has NO effect on which tools reach the LLM. `McpTool` overrides this
    /// to return `mcp__{sanitized_server_name}` so the catalog can group MCP
    /// tools per server for display while `toolset()` keeps reporting the flat
    /// [`MCP_TOOLSET`] sentinel every LLM-facing filter still keys on.
    fn display_group(&self) -> Option<&str> {
        None
    }

    /// Phase 48.2 Plan 11 (G-48.2-6 slice a): the stable identifier of a
    /// SEPARATE process that executes this tool's effects, for a tool that
    /// still works — `is_available()` unaffected — without that process
    /// running. `None` (the default) means no such dependency exists; every
    /// existing `impl Tool` inherits this with zero edits, the same
    /// zero-edit guarantee `credentials()`/`display_group()` above were
    /// built on.
    ///
    /// This is DELIBERATELY not `prerequisites()`. A prerequisite that is
    /// unsatisfied makes `is_available()` return `false`; `cronjob` with the
    /// gateway down is genuinely available — the operator can create,
    /// pause, or remove a schedule and it persists correctly, it just will
    /// not FIRE until the gateway process runs again. A `required: true`
    /// prereq here would render a false UNAVAILABLE; a `required: false`
    /// one would never be reported at all, because
    /// [`unsatisfied_prerequisites`] filters on `required` (see its doc
    /// comment above). Neither is the fact this method carries — it is
    /// additive, non-gating information about WHO executes the tool's
    /// effects, not whether the tool can accept them right now. The next
    /// implementer tempted to fold this into `prerequisites()` should
    /// re-read this paragraph first: doing so reintroduces one of the two
    /// bugs just described.
    fn runtime_dependency(&self) -> Option<&'static str> {
        None
    }

    /// Phase 25.3 D-T-1 / Discretion D-2: redact sensitive values from raw tool args
    /// before they are recorded in the trajectory ledger.
    ///
    /// Default: return args unchanged (most tools have no secrets in their args).
    /// Tools that handle credentials override this — e.g., `WebExtractTool` calls
    /// `crate::web_extract::sanitize::redact_secrets_in_url()` (Phase 25.2 Plan 16)
    /// on URL-typed args.
    ///
    /// The TrajectoryWriter (Phase 25.3 D-T-2) calls `tool.redact_args(&raw_args)`
    /// before serializing the entry — see Plan 9 AgentLoop callback wireup.
    ///
    /// Contract: the returned Value is what lands in `TrajectoryEntry.args`. It MUST
    /// preserve the structural shape (object/array/scalar) so downstream consumers
    /// (Phase 25.4 Curator, RL pipelines) can count fields. Only string LEAVES that
    /// contain secrets should be replaced with redacted placeholders.
    fn redact_args(&self, raw: &serde_json::Value) -> serde_json::Value {
        raw.clone()
    }

    /// D-13 (Phase 27.1.1): called on REPL/CLI/TUI session end so tools
    /// can fire a final cleanup. Default is a synchronous no-op — all
    /// 25 existing `impl Tool` blocks get this for free (zero breaking
    /// changes). Override to run fire-and-forget cleanup; per D-14 the
    /// override uses `tokio::spawn(async move { ... })` internally so
    /// the trait method itself stays `fn` (not `async fn`) and the
    /// trait remains object-safe for `Box<dyn Tool>`.
    fn on_session_end(&self) {}

    /// D-04/D-05 (Phase 41.3): wall-clock execution budget for this tool, in
    /// seconds. Default returns `Some(DEFAULT_TOOL_TIMEOUT_SECS)` — a tool
    /// that declares nothing still inherits a bound. This is a deliberate
    /// inversion of the Python parity behavior (where a tool was unbounded
    /// *by omission*): here, opting out of the bound is one greppable,
    /// reviewable line in the tool's own impl (`Some(0)` or `None`).
    /// `resolve_tool_timeout` governs how this interacts with operator
    /// config (D-06). Mirrors the `on_session_end` default-impl precedent
    /// immediately above — all existing `impl Tool` blocks get correct
    /// behavior with zero edits.
    fn timeout_secs(&self) -> Option<u64> {
        Some(DEFAULT_TOOL_TIMEOUT_SECS)
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String>;
}

/// D-06 (Phase 41.3): resolve the effective wall-clock timeout for `tool`,
/// highest-precedence first:
///
/// 1. `tools_cfg.timeout_overrides[name]` — operator, per-tool. A value `> 0`
///    uses that many seconds; a value `<= 0` disables the bound. This arm
///    **wins over a code-level `None` opt-out** — checked before the tool is
///    even consulted — so a wedged opted-out tool can be capped from
///    `config.yaml` with no rebuild.
/// 2. `tool.timeout_secs()` — the tool's own declared budget. `Some(n)` with
///    `n > 0` uses `n`; `Some(0)` or `None` means the tool opted out.
/// 3. `tools_cfg.timeout_secs` — operator global default. The trait default
///    body already returns `Some(DEFAULT_TOOL_TIMEOUT_SECS)` for every
///    non-declaring tool, so this level is made reachable by comparing
///    `tool.timeout_secs()` against that exact constant: an exact match means
///    "did not declare its own budget", and the operator's global default
///    applies instead of the trait constant.
/// 4. `DEFAULT_TOOL_TIMEOUT_SECS` — the floor, reached when `tools_cfg.timeout_secs`
///    is itself unconfigured (defaults to the same constant).
///
/// Reads `tools_cfg` fresh on every call (per the `Config::load()`-at-call-time
/// idiom used throughout this crate) — never cache it on the registry.
pub fn resolve_tool_timeout(
    tool: &dyn Tool,
    name: &str,
    tools_cfg: &ironhermes_core::config::ToolsConfig,
) -> Option<Duration> {
    // Level 1: per-tool operator override — wins over a code-level `None`,
    // and is checked before the tool is consulted at all.
    if let Some(&secs) = tools_cfg.timeout_overrides.get(name) {
        return if secs > 0 {
            Some(Duration::from_secs(secs as u64))
        } else {
            None
        };
    }

    // Level 2: the tool's own declared budget.
    match tool.timeout_secs() {
        Some(0) | None => None,
        Some(n) if n != DEFAULT_TOOL_TIMEOUT_SECS => Some(Duration::from_secs(n)),
        // Level 3/4: the tool did not declare (its `timeout_secs()` returned
        // exactly the trait-default constant) — the operator's global
        // default is live here, itself defaulting to the same floor.
        _ => Some(Duration::from_secs(tools_cfg.timeout_secs)),
    }
}

/// D-12 / D-14 (Phase 25): async handler for intercepted tools.
/// Resolves Open Question 3 — async because execute_tool_call() is async; spawn_blocking
/// for sync-StateStore stays inside the closure body (see session_search migration in Plan 3).
///
/// Security (T-25-04): library-internal use only — handler closures MUST be constructed
/// by the workspace (ironhermes-agent::AgentLoop::with_intercepts), NOT deserialized from
/// config or user input. The `with_intercepts(...)` builder in Plan 3 accepts only the five
/// known-safe handles (memory_manager, state_store, subagent_runner, todo_state, cron_router).
pub type InterceptHandler = std::sync::Arc<
    dyn Fn(serde_json::Value) -> futures::future::BoxFuture<'static, anyhow::Result<String>>
        + Send
        + Sync,
>;

/// Phase 36.3.12 CR-01: sentinel session key used by `register_intercepted` — the
/// pre-CR-01 API that never took a session_id (session_search / memory / delegate_task
/// / todo_read / todo_write, wired once per `AgentLoop::with_intercepts` builder call).
/// Handlers registered this way are session-agnostic by design: `dispatch_intercepts`
/// falls back to this key ONLY when the caller's real session_id misses an exact match.
/// `register_intercepted_or_replace` (the session-scoped gateway D-08/D-10 API for
/// "terminal"/"execute_code") NEVER inserts under this key, so the fallback never
/// masks a genuine session-lookup miss on those two names — the fail-closed guarantee
/// holds exactly where CR-01 requires it.
const LEGACY_GLOBAL_INTERCEPT_SESSION: &str = "__legacy_global_intercept_session__";

/// Phase 48.2 Plan 01 (D-16/D-20): one row of the Tools page's UNFILTERED
/// catalog — every registered tool, regardless of toolset-enabled state,
/// availability, or the per-tool `disabled` list. `toolset` is the
/// registry's LLM-facing filtering identity ([`Tool::toolset`], `"mcp"` for
/// every MCP tool); `group` is the display identity ([`Tool::display_group`]
/// when `Some`, otherwise equal to `toolset`). Keeping both fields is
/// deliberate — the operator's mental model (per-server MCP groups) and the
/// LLM's filtering axis (the flat `"mcp"` sentinel) are two different things,
/// and collapsing them back into one field is exactly the mistake this type
/// exists to avoid.
#[derive(Debug, Clone)]
pub struct ToolCatalogRow {
    pub name: String,
    pub description: String,
    pub toolset: String,
    pub group: String,
    pub available: bool,
    pub missing_prerequisites: Vec<Prerequisite>,
    /// Phase 48.2 Plan 11 (G-48.2-6 slice a): [`Tool::runtime_dependency`]'s
    /// answer, carried through unchanged. `Some(GATEWAY_RUNTIME_DEPENDENCY)`
    /// for `cronjob`; `None` for every tool with no such dependency
    /// (including `delegate_task`, which runs in-process).
    pub runtime_dependency: Option<&'static str>,
}

pub struct ToolRegistry {
    /// Phase 32.1-06: changed from `Box<dyn Tool>` to `Arc<dyn Tool>` to enable
    /// `scope_to()` (which constructs a filtered clone without a Clone bound on dyn Tool).
    /// The public `register` / `register_dynamic` API still accepts `Box<dyn Tool>`;
    /// we convert to Arc on insert so all existing callers are unchanged.
    tools: HashMap<String, Arc<dyn Tool>>,
    guardrails: Vec<Box<dyn ironhermes_hooks::GuardrailHook>>,
    error_detail: ironhermes_hooks::ErrorDetailLevel,
    /// D-14 (Phase 25): intercepted tools stored separately from regular tools.
    /// get_definitions() returns schemas from BOTH maps; D-15 prevents name collisions.
    ///
    /// Phase 36.3.12 CR-01: the inner value is now name-major, session-minor —
    /// `(ToolSchema, HashMap<session_id, InterceptHandler>)` — instead of a single
    /// handler per name. On the ONE shared `Arc<AgentRuntime>` the gateway serves every
    /// chat from, a flat name-keyed handler let one session's turn overwrite another's
    /// closure mid-flight, misattributing the audit `session_id`/`chat_id` and routing
    /// the approval prompt to the wrong operator. `register_intercepted_or_replace` is
    /// session-scoped and `dispatch_intercepts` fails closed (never falls through) on a
    /// session-lookup miss for a name that IS intercepted. `register_intercepted`
    /// (session-agnostic legacy callers — session_search/memory/delegate_task/todo_*)
    /// stores under `LEGACY_GLOBAL_INTERCEPT_SESSION`, which `dispatch_intercepts` uses
    /// as a fallback ONLY when the exact session_id misses — so those tools keep
    /// working for every session while "terminal"/"execute_code" stay strictly
    /// session-scoped and fail-closed (see 36.3.12-REVIEW.md CR-01).
    intercepts: HashMap<String, (ToolSchema, HashMap<String, InterceptHandler>)>,
    /// D-22 (Phase 25): optional toolset configuration.
    /// When Some, get_definitions() applies toolset-level filtering (D-23).
    /// When None, no toolset filter is applied — preserves pre-Phase-25 behavior (A2/Pitfall 8).
    toolset_config: Option<ironhermes_core::config::ToolsConfig>,
    /// Phase 36.17.7 D-06 (Path B — REVISION BLOCKER 2): the SessionKey that
    /// last invoked `register_tts_tools` on this registry. Used by
    /// `is_tts_registered_live` and `tts_registration_status` to distinguish:
    ///   - `Live` — a real per-turn session_key (Web/Telegram/TUI Local
    ///     non-sentinel) is the registered owner.
    ///   - `Inspection` — the sentinel `(Platform::Local, "inspect")` set by
    ///     `register_tts_for_inspection` (CLI inspection path).
    ///   - `NotRegistered` — `register_tts_tools` never fired.
    ///
    /// `None` here means TTS tools were never registered; `Some(key)` is set
    /// every time `register_tts_tools` is called. Path B avoids adding `Any`
    /// to the `Tool` trait — the SessionKey lives directly on the registry.
    tts_session_key: Option<ironhermes_core::SessionKey>,
    /// Phase 36.17.8: STT registry for the voice capture loop (Plan 05) and
    /// web server (Plan 06). NOT an LLM tool — carried on the registry struct
    /// so downstream code can obtain the same Arc without re-constructing it.
    /// `None` until `register_stt_registry()` is called.
    pub stt_registry: Option<Arc<ironhermes_core::SttRegistry>>,
    /// Skill-as-tool fallback: a handle to the same `SkillRegistry` owned by the
    /// `skills` tool. When the LLM calls a bare skill name (e.g. `arxiv`) instead
    /// of `skills(action="activate", name="arxiv")` — a common failure mode for
    /// weaker local models — dispatch resolves the name against this registry and
    /// reroutes the call to the `skills` tool. `None` until `register_skills_tool()`
    /// is called, so the fallback is a no-op when the skills toolset is disabled.
    skill_registry: Option<Arc<ironhermes_core::SkillRegistry>>,
    /// Phase 01 Plan 03 (SAFE-05 / D-03): the shared, `SessionKey`-keyed
    /// generation counter for `image_gen`. Lives on the registry so that
    /// every per-session `ImageGenTool` registered via
    /// `register_image_gen_tool` shares one counter — the cap is
    /// scoped per `SessionKey`, so one chat hitting the limit never blocks
    /// another (T-03-02). Initialized empty in `new()`.
    image_gen_counter: crate::image_gen::SessionCounter,
    /// Phase 36.3.3 Plan 02: the shared, `SessionKey`-keyed generation counter
    /// for `video_gen` / `video_animate`. Kept separate from `image_gen_counter`
    /// (Pitfall 8) so the video cap (5 per session, D-06) never interferes with
    /// the image cap. Initialized empty in `new()`.
    video_gen_counter: crate::video_gen::VideoSessionCounter,
    /// Phase 41.3 D-03: monotonic counter of `execute_tool` calls that
    /// expired at their resolved D-06 budget. Read via `timeout_count()`.
    /// Log + metric only — no new UI, no `/agents` integration.
    timeout_counter: Arc<AtomicU64>,
    /// Phase 41.3 Plan 11 (D-19): the resolved env → config → vault
    /// credential snapshot this registry hands to credential-bearing tools
    /// it constructs (`register_defaults_except`, `register_delegate_task_tool`).
    /// Defaults to an env-only snapshot in `new()` — every existing call site
    /// that never calls `with_credentials` keeps today's env-lookup-only
    /// behavior (`ToolCredentials::env_only()`'s doc comment). Set by the
    /// production async seam (`build_app_runtime_bundle`) before any tool is
    /// registered.
    credentials: Arc<crate::credentials::ToolCredentials>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            guardrails: Vec::new(),
            error_detail: ironhermes_hooks::ErrorDetailLevel::Full,
            intercepts: HashMap::new(),
            toolset_config: None,
            // Phase 36.17.7 D-06: TTS session key is set by `register_tts_tools`.
            tts_session_key: None,
            // Phase 36.17.8: STT registry — None until register_stt_registry() called.
            stt_registry: None,
            // Skill-as-tool fallback — set by register_skills_tool().
            skill_registry: None,
            // Phase 01 Plan 03: empty per-session image_gen cap counter.
            image_gen_counter: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            // Phase 36.3.3 Plan 02: empty per-session video_gen cap counter.
            video_gen_counter: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            // Phase 41.3 D-03: zeroed timeout-expiry counter.
            timeout_counter: Arc::new(AtomicU64::new(0)),
            // Phase 41.3 Plan 11 (D-19): env-only snapshot until with_credentials
            // installs the real one — env lookups stay live either way.
            credentials: Arc::new(crate::credentials::ToolCredentials::env_only()),
        }
    }

    /// Phase 41.3 Plan 11 (D-19): install the resolved credential snapshot.
    /// Must be called before `register_defaults_except` / any credential-bearing
    /// tool construction for the snapshot to reach that tool — the production
    /// seam (`build_app_runtime_bundle`) calls this immediately after
    /// `ToolRegistry::new()`.
    pub fn with_credentials(&mut self, credentials: Arc<crate::credentials::ToolCredentials>) {
        self.credentials = credentials;
    }

    /// Phase 41.3 Plan 11 (D-19): a clone of this registry's resolved
    /// credential snapshot — the same `Arc` every credential-bearing tool
    /// this registry constructs was handed.
    pub fn credentials(&self) -> Arc<crate::credentials::ToolCredentials> {
        self.credentials.clone()
    }

    /// Phase 36.17.7 D-06 (Path B — REVISION BLOCKER 2): returns the current
    /// TTS registration status of this registry.
    ///
    /// - `Live` when `text_to_speech` AND `send_audio` are registered AND the
    ///   recording session_key is NOT the sentinel `(Platform::Local, "inspect")`.
    /// - `Inspection` when both tools are registered AND the session_key is
    ///   the sentinel.
    /// - `NotRegistered` otherwise (either tool missing OR `register_tts_tools`
    ///   was never called).
    pub fn tts_registration_status(
        &self,
    ) -> ironhermes_core::commands::context::TtsRegistrationStatus {
        use ironhermes_core::commands::context::TtsRegistrationStatus;
        if !self.tools.contains_key("text_to_speech") || !self.tools.contains_key("send_audio") {
            return TtsRegistrationStatus::NotRegistered;
        }
        let Some(key) = self.tts_session_key.as_ref() else {
            return TtsRegistrationStatus::NotRegistered;
        };
        if key.platform == ironhermes_core::types::Platform::Local && key.chat_id == "inspect" {
            TtsRegistrationStatus::Inspection
        } else {
            TtsRegistrationStatus::Live
        }
    }

    /// Phase 36.17.7 D-06 (Path B — REVISION BLOCKER 2): returns `true` iff
    /// `tts_registration_status() == Live` — i.e. TTS tools are registered AND
    /// the session_key is NOT the inspection sentinel.
    pub fn is_tts_registered_live(&self) -> bool {
        matches!(
            self.tts_registration_status(),
            ironhermes_core::commands::context::TtsRegistrationStatus::Live
        )
    }

    /// Set the toolset configuration for runtime filtering (D-22, Phase 25).
    /// Pass `None` to disable toolset filtering (preserves pre-Phase-25 behavior, A2/Pitfall 8).
    pub fn set_toolset_config(&mut self, cfg: Option<ironhermes_core::config::ToolsConfig>) {
        self.toolset_config = cfg;
    }

    /// Phase 41.3 D-03: number of `execute_tool` calls abandoned at their
    /// resolved D-06 budget so far. Log + metric only observability — no new
    /// UI, no `/agents` integration.
    pub fn timeout_count(&self) -> u64 {
        self.timeout_counter.load(Ordering::Relaxed)
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        assert!(
            !self.intercepts.contains_key(&name),
            "register: name '{}' already registered as an intercepted tool — schema duplication blocked at registry build (D-15)",
            name,
        );
        self.tools.insert(name, Arc::from(tool));
    }

    /// Register a tool dynamically (e.g., from MCP discovery). Per D-10.
    /// Functionally identical to register() -- the name distinction is semantic
    /// (dynamic = runtime MCP vs static = startup built-in).
    /// D-15: Also guards against intercept name collisions for MCP-discovered tools.
    pub fn register_dynamic(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        assert!(
            !self.intercepts.contains_key(&name),
            "register_dynamic: name '{}' already registered as an intercepted tool — schema duplication blocked at registry build (D-15)",
            name,
        );
        self.tools.insert(name, Arc::from(tool));
    }

    /// Register an intercepted tool by name, schema, and async handler (D-12 / D-14, Phase 25).
    ///
    /// Intercepted tools are NOT in the regular `tools` HashMap; they live in a separate
    /// `intercepts` map. `get_definitions()` returns schemas from BOTH maps so the LLM sees
    /// the full surface, but `dispatch_intercepts()` handles them before `dispatch()` is called.
    ///
    /// D-15 reciprocal guard: panics if `name` is already registered as a regular tool —
    /// schema duplication is structurally impossible.
    ///
    /// Security (T-25-04): library-internal use only — handler closures MUST be constructed
    /// by the workspace (ironhermes-agent::AgentLoop::with_intercepts), NOT deserialized from
    /// config or user input.
    pub fn register_intercepted(
        &mut self,
        name: &str,
        schema: ToolSchema,
        handler: InterceptHandler,
    ) {
        assert!(
            !self.tools.contains_key(name),
            "register_intercepted: name '{}' already registered as a regular tool — schema duplication blocked at registry build (D-15)",
            name,
        );
        let mut sessions = HashMap::new();
        sessions.insert(LEGACY_GLOBAL_INTERCEPT_SESSION.to_string(), handler);
        self.intercepts.insert(name.to_string(), (schema, sessions));
    }

    /// Phase 45 / Phase 36.3.12 CR-01: Per-turn, per-session intercept override —
    /// register or replace a SESSION-SCOPED gated intercepted version of a tool.
    /// Allows the gateway to route LLM-issued `terminal`/`execute_code` calls through
    /// a gating closure (DangerousCommandGuardrail + ApprovalGate / execute_gated_command)
    /// WITHOUT one chat's turn overwriting another chat's closure (CR-01: on the single
    /// shared `Arc<AgentRuntime>` the gateway serves every chat from, a flat name-keyed
    /// handler let chat A's command dispatch through chat B's closure, misattributing the
    /// audit session_id/chat_id and routing the approval prompt to the wrong operator).
    ///
    /// Behaviour:
    /// - Name already in `intercepts` map → insert/replace the handler under `session_id`
    ///   in the inner map; the stored schema is left untouched.
    /// - Name still in `tools` map → steal its schema, remove from `tools`, insert into
    ///   `intercepts` with a fresh inner map containing this one `session_id`.
    /// - Neither → insert with `schema_fallback` and a fresh inner map containing this
    ///   one `session_id` (used when the tool isn't registered at all).
    pub fn register_intercepted_or_replace(
        &mut self,
        name: &str,
        session_id: &str,
        schema_fallback: ironhermes_core::ToolSchema,
        handler: InterceptHandler,
    ) {
        if let Some((_schema, sessions)) = self.intercepts.get_mut(name) {
            sessions.insert(session_id.to_string(), handler);
        } else if let Some(regular) = self.tools.remove(name) {
            let schema = regular.schema();
            let mut sessions = HashMap::new();
            sessions.insert(session_id.to_string(), handler);
            self.intercepts.insert(name.to_string(), (schema, sessions));
        } else {
            let mut sessions = HashMap::new();
            sessions.insert(session_id.to_string(), handler);
            self.intercepts
                .insert(name.to_string(), (schema_fallback, sessions));
        }
    }

    /// Dispatch a tool call to the intercepts map (D-12, Phase 25; session-scoped per
    /// Phase 36.3.12 CR-01).
    ///
    /// Returns `Some(result)` when the tool name is intercepted; `None` to fall through
    /// to the normal `dispatch()` path. The agent_loop call site is responsible for:
    /// ```rust,ignore
    /// if let Some(r) = registry.dispatch_intercepts(name, session_id, args.clone()).await {
    ///     return r;
    /// }
    /// registry.dispatch(name, args).await
    /// ```
    ///
    /// Semantics (CR-01 fail-closed):
    /// - `name` absent from `intercepts` → `None` (ordinary tool — caller falls through).
    /// - `name` present, `session_id` present in the inner map → invoke that session's
    ///   handler.
    /// - `name` present, `session_id` absent, but the legacy session-agnostic sentinel
    ///   (`LEGACY_GLOBAL_INTERCEPT_SESSION`, populated only by `register_intercepted`) IS
    ///   present → invoke it. This keeps session_search/memory/delegate_task/todo_* working
    ///   for every session; `register_intercepted_or_replace` never populates this key, so
    ///   it never masks a genuine miss on "terminal"/"execute_code".
    /// - `name` present, neither `session_id` nor the legacy sentinel present →
    ///   `Some(Err(..))`. This is FAIL-CLOSED and is the single most important behavior
    ///   here: `register_intercepted_or_replace` permanently removes the tool from the
    ///   `tools` map, so returning `None` would make the agent loop fall through to
    ///   `dispatch()` — and any future change that restores the raw tool to `tools` would
    ///   silently resurrect the ungated path CR-01 closes. Returning `Some(Err(..))` makes
    ///   that structurally impossible.
    pub async fn dispatch_intercepts(
        &self,
        name: &str,
        session_id: &str,
        args: serde_json::Value,
    ) -> Option<anyhow::Result<String>> {
        let (_schema, sessions) = self.intercepts.get(name)?;
        match sessions
            .get(session_id)
            .or_else(|| sessions.get(LEGACY_GLOBAL_INTERCEPT_SESSION))
        {
            Some(handler) => Some(handler(args).await),
            None => Some(Err(anyhow::anyhow!(
                "intercepted tool '{name}' has no handler registered for session '{session_id}' \
                 — fail-closed (CR-01), refusing to fall through to the raw ungated tool"
            ))),
        }
    }

    /// Phase 36.3.12 CR-01: evict a session's handlers from every intercepted tool name.
    /// Drops any name whose inner map becomes empty as a result (this does NOT restore
    /// the name to the regular `tools` map — the tool's `Arc` was already consumed by
    /// `register_intercepted_or_replace`; the name is simply gone from `intercepts` too).
    ///
    /// Without this, every session that ever ran a gated turn would retain its captured
    /// closure (which holds `Arc<Config>`, the tool `Arc`, and the gate `Arc`) for the
    /// process lifetime — unbounded memory growth on a long-lived gateway (T-36.3.12-CR01-D).
    pub fn unregister_intercepts_for_session(&mut self, session_id: &str) {
        self.intercepts.retain(|_name, (_schema, sessions)| {
            sessions.remove(session_id);
            !sessions.is_empty()
        });
    }

    /// Phase 36.3.12 WR-05: introspection accessor — number of distinct sessions
    /// currently holding a session-scoped intercept handler for `name` (0 if `name`
    /// is not intercepted at all). Exists so cross-crate regression tests (this
    /// method is called from `ironhermes-agent`'s `run_turn` leak-regression test,
    /// which cannot reach the private `intercepts` field directly) can assert the
    /// map does not grow unbounded across turns/sessions without needing a
    /// dispatch-based proxy. Always compiled (not `#[cfg(test)]`): a private field
    /// gated by `#[cfg(test)]` in this crate is invisible when this crate is
    /// compiled as a dependency of another crate's test binary.
    pub fn intercept_session_count(&self, name: &str) -> usize {
        self.intercepts
            .get(name)
            .map(|(_schema, sessions)| sessions.len())
            .unwrap_or(0)
    }

    /// Returns (tool_name, [unsatisfied required prereqs]) for every Tool whose
    /// required prereqs are missing. Used by Plan 5's preflight banner (D-17).
    /// Only checks `kind == "env_var"` at the trait level; `kind == "config_field"`
    /// is checked at config-load, not here (D-08 / D-09).
    pub fn list_unavailable(&self) -> Vec<(String, Vec<Prerequisite>)> {
        self.tools
            .values()
            .filter_map(|t| {
                let missing: Vec<_> = t
                    .prerequisites()
                    .into_iter()
                    .filter(|p| {
                        p.required
                            && match p.kind.as_str() {
                                "env_var" => std::env::var(&p.name).is_err(),
                                _ => false, // config_field handled at config-load layer
                            }
                    })
                    .collect();
                if missing.is_empty() {
                    None
                } else {
                    Some((t.name().to_string(), missing))
                }
            })
            .collect()
    }

    /// Unique set of toolset() values across all currently-registered tools, sorted alphabetically.
    /// Per D-03: membership is read at runtime from the trait, no separate table.
    /// Return a new `ToolRegistry` that contains only the tools whose
    /// `toolset()` value is listed in `toolset_names`.
    ///
    /// Added in Phase 32.1 Plan 06 to implement
    /// D-CONTEXT §Per-job runtime overrides: "enabled_toolsets ⇒ scoped tool registry".
    /// The original `Arc<RwLock<ToolRegistry>>` is NOT mutated — callers get an
    /// independent view. Guardrails, intercepts, and `toolset_config` are NOT
    /// carried over (the scoped registry is lightweight; callers may re-apply
    /// them if needed).
    pub fn scope_to(&self, toolset_names: &[String]) -> Self {
        let allowed: std::collections::HashSet<&str> =
            toolset_names.iter().map(|s| s.as_str()).collect();
        let tools = self
            .tools
            .iter()
            .filter(|(_, t)| {
                // Dynamic MCP tools (toolset() == MCP_TOOLSET) are exempt: they are not part
                // of the built-in toolset taxonomy, so no caller-supplied allowlist entry
                // exists for "mcp" and a strict `allowed.contains(...)` check would silently
                // drop every discovered MCP tool from subagent/kanban-worker registries even
                // when a server is connected. Their gating lives in MCP-server connection +
                // McpMutationGuardrail + the per-tool `disabled` list, mirroring the
                // get_definitions() exemption above (D-09a, Phase 46).
                t.toolset() == MCP_TOOLSET || allowed.contains(t.toolset())
            })
            .map(|(k, t)| (k.clone(), t.clone()))
            .collect();
        Self {
            tools,
            guardrails: Vec::new(),
            error_detail: self.error_detail.clone(),
            intercepts: HashMap::new(),
            toolset_config: None,
            // Phase 36.17.7 D-06: scoped registry inherits the parent's TTS
            // session_key so `tts_registration_status` continues to reflect
            // the original registration provenance.
            tts_session_key: self.tts_session_key.clone(),
            // Phase 36.17.8: scoped registry inherits parent's STT registry.
            stt_registry: self.stt_registry.clone(),
            // Scoped registry inherits the parent's SkillRegistry handle so the
            // skill-as-tool fallback keeps working after scoping. (If the `skills`
            // tool itself is scoped out, the fallback's get("skills") miss makes
            // it degrade gracefully to the normal "Tool not found" error.)
            skill_registry: self.skill_registry.clone(),
            // Phase 01 Plan 03: scoped registry shares the parent's image_gen
            // cap counter so the per-session limit is honored after scoping.
            image_gen_counter: self.image_gen_counter.clone(),
            // Phase 36.3.3 Plan 02: scoped registry shares the parent's video_gen
            // cap counter so the per-session video limit is honored after scoping.
            video_gen_counter: self.video_gen_counter.clone(),
            // Phase 41.3 D-03: scoped registry shares the parent's timeout
            // counter so expiries are not silently under-counted after scoping.
            timeout_counter: self.timeout_counter.clone(),
            // Phase 41.3 Plan 11 (D-19): scoped registry inherits the parent's
            // credential snapshot — a scoped clone with a blank snapshot would
            // silently degrade a credential-backed tool exactly the way a
            // dropped MCP tool did before the exemption above (D-09a).
            credentials: self.credentials.clone(),
        }
    }

    /// Only includes regular tools (from the `tools` map); intercepted tools have no Tool::toolset()
    /// method. Plan 4's `hermes toolset list` presents intercepted-only names through a separate path.
    pub fn list_toolsets(&self) -> Vec<String> {
        let mut s: std::collections::HashSet<String> = self
            .tools
            .values()
            .map(|t| t.toolset().to_string())
            .collect();
        let mut v: Vec<String> = s.drain().collect();
        v.sort();
        v
    }

    /// Phase 48.2 Plan 01 (D-16/D-20, RESEARCH Pitfall 1): the UNFILTERED
    /// tool catalog for the web Tools page — one [`ToolCatalogRow`] per
    /// registered tool, sorted by `(group, name)`. Deliberately applies NO
    /// toolset filter, NO availability filter, and NO per-tool disable
    /// filter — a card the operator must fix has to reach the browser, which
    /// is the entire reason this read path exists as a sibling to
    /// [`Self::get_definitions`] rather than reusing it. Intercepted tools
    /// are out of scope for this row set, matching [`Self::list_toolsets`]'s
    /// documented limitation (no `Tool::toolset()` for intercepts).
    pub fn catalog_rows(&self) -> Vec<ToolCatalogRow> {
        let mut rows: Vec<ToolCatalogRow> = self
            .tools
            .values()
            .map(|t| {
                let toolset = t.toolset().to_string();
                let group = t
                    .display_group()
                    .map(|g| g.to_string())
                    .unwrap_or_else(|| toolset.clone());
                ToolCatalogRow {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    toolset,
                    group,
                    available: t.is_available(),
                    missing_prerequisites: unsatisfied_prerequisites(t.as_ref()),
                    runtime_dependency: t.runtime_dependency(),
                }
            })
            .collect();
        rows.sort_by(|a, b| (a.group.as_str(), a.name.as_str()).cmp(&(b.group.as_str(), b.name.as_str())));
        rows
    }

    /// Remove all tools whose name starts with `{server_name}__`.
    /// Called on /reload-mcp to clear one server's tools before re-registering.
    /// Returns the number of tools removed.
    pub fn unregister_by_prefix(&mut self, server_name: &str) -> usize {
        let prefix = format!("{server_name}__");
        let before = self.tools.len();
        self.tools.retain(|name, _| !name.starts_with(&prefix));
        before - self.tools.len()
    }

    /// Phase 36.3.7.13 D-B2: retain only tools whose name is in `allowed`.
    ///
    /// Called by `filter_for_goal_mode_if_applicable` (main.rs) after
    /// `register_kanban_tools_if_applicable` to enforce the goal-mode toolset
    /// preset (D-B1). Tools NOT in `allowed` are permanently removed from the
    /// registry for this worker session — the LLM never sees their schemas
    /// (D-B2 security contract).
    ///
    /// Returns the number of tools removed (mirrors `unregister_by_prefix`
    /// return contract for symmetric API).
    ///
    /// # Safety
    ///
    /// This is a destructive operation on `self.tools`. It MUST NOT be called
    /// in interactive session paths (REPL / TUI) — only in kanban worker-mode
    /// entry where the toolset is fixed for the entire session lifetime.
    pub fn retain_by_name(&mut self, allowed: &[&str]) -> usize {
        let before = self.tools.len();
        self.tools
            .retain(|name, _| allowed.contains(&name.as_str()));
        before - self.tools.len()
    }

    /// Add a guardrail hook that will be checked before every tool dispatch.
    /// Guardrails are checked in registration order.
    /// Per D-05: register BlocklistGuardrail first, custom trait hooks second.
    pub fn add_guardrail(&mut self, hook: Box<dyn ironhermes_hooks::GuardrailHook>) {
        self.guardrails.push(hook);
    }

    /// Set the error detail level for guardrail block messages.
    pub fn set_error_detail(&mut self, level: ironhermes_hooks::ErrorDetailLevel) {
        self.error_detail = level;
    }

    /// Phase 36.3.12 D-08/D-10: fetch a clone of an already-registered REGULAR
    /// tool's `Arc`, for callers that need to invoke the tool's real dispatch logic
    /// from a gating wrapper built OUTSIDE the registry (`ironhermes-agent`'s
    /// `run_turn` captures these once, at `AgentRuntime::from_config` time —
    /// see `AgentRuntime::terminal_tool_arc`/`execute_code_tool_arc`) — BEFORE the
    /// per-turn `register_intercepted_or_replace("terminal"/"execute_code", ...)`
    /// call permanently moves the name out of the `tools` map on turn 1. Returns
    /// `None` if `name` isn't in the regular tools map (already intercepted, or
    /// never registered).
    pub fn get_arc(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Returns the LLM-visible schema list with all currently-configured filters applied.
    ///
    /// Filter resolution order (D-23):
    /// 1. If `toolset_config` is `Some(_)` and a tool's toolset is disabled → exclude.
    ///    If `toolset_config` is `None`, no toolset filter is applied (pre-Phase-25 behavior, A2/Pitfall 8).
    /// 2. If `is_available()` returns `false` (missing required prerequisites) → exclude.
    /// 3. If `enabled_tools` is `Some(list)` → narrow to that list.
    /// 4. Schemas from both `tools` and `intercepts` maps are unioned.
    pub fn get_definitions(&self, enabled_tools: Option<&[String]>) -> Vec<ToolSchema> {
        let mut schemas: Vec<ToolSchema> = self
            .tools
            .values()
            .filter(|t| {
                // D-23 layer 1: toolset-level filter (only when toolset_config is set).
                // When None, no toolset filter is applied (pre-Phase-25 behavior, A2/Pitfall 8).
                //
                // Dynamic MCP tools (toolset() == MCP_TOOLSET) are exempt: they are not part
                // of the built-in toolset taxonomy, so no config entry exists and
                // is_toolset_enabled("mcp") fails closed → every discovered MCP tool would be
                // hidden from the LLM (Phase 45 UAT #3 blocker). Their gating lives in MCP-server
                // connection + McpMutationGuardrail + the per-tool `disabled` list (layer 4).
                //
                // Kanban worker-protocol tools (toolset() == KANBAN_TOOLSET) are exempt for the
                // same reason: "kanban" is not in ALL_TOOLSETS or any config, so a fail-closed
                // filter hid kanban_complete/kanban_block from workers, stranding every worker
                // with no terminator. They self-gate via is_available() (worker env OR
                // explicit-enable), so exemption keeps visibility correctly scoped.
                t.toolset() == MCP_TOOLSET
                    || t.toolset() == KANBAN_TOOLSET
                    || self
                        .toolset_config
                        .as_ref()
                        .map(|cfg| cfg.is_toolset_enabled(t.toolset()))
                        .unwrap_or(true)
            })
            .filter(|t| t.is_available())
            .filter(|t| {
                // D-23 layer 3: enabled_tools list filter.
                enabled_tools
                    .map(|list| list.iter().any(|name| name == t.name()))
                    .unwrap_or(true)
            })
            .filter(|t| {
                // D-23 layer 4: per-tool disabled list within an enabled toolset.
                self.toolset_config
                    .as_ref()
                    .map(|cfg| !cfg.disabled.iter().any(|d| d == t.name()))
                    .unwrap_or(true)
            })
            .map(|t| t.schema())
            .collect();
        // Phase 25 D-14: union with intercept schemas.
        // Apply enabled_tools filter + intercepted_owner_toolset toolset filter + disabled list.
        schemas.extend(
            self.intercepts
                .iter()
                .filter(|(name, _)| {
                    // D-23 layer 1 for intercepts: check owner toolset.
                    self.toolset_config
                        .as_ref()
                        .map(|cfg| cfg.is_toolset_enabled(intercepted_owner_toolset(name)))
                        .unwrap_or(true)
                })
                .filter(|(name, _)| {
                    // D-23 layer 3: enabled_tools filter.
                    enabled_tools
                        .map(|list| list.iter().any(|n| n == name.as_str()))
                        .unwrap_or(true)
                })
                .filter(|(name, _)| {
                    // D-23 layer 4: per-tool disabled list.
                    self.toolset_config
                        .as_ref()
                        .map(|cfg| !cfg.disabled.iter().any(|d| d.as_str() == name.as_str()))
                        .unwrap_or(true)
                })
                .map(|(_, (schema, _))| schema.clone()),
        );
        schemas
    }

    pub async fn dispatch(&self, name: &str, args: serde_json::Value) -> anyhow::Result<String> {
        self.dispatch_with_hook(name, args, None::<fn(&str, &str)>)
            .await
    }

    /// Check all registered guardrails for the given tool call WITHOUT executing the tool.
    ///
    /// Returns the first non-Allow decision (Block wins immediately), or Allow if all
    /// guardrails pass. Warn decisions are returned as-is — the caller decides whether
    /// to log or surface them.
    ///
    /// Used by agent_loop.rs to implement D-05 ordering:
    ///   check_guardrails → (Block → ToolCompleted{false}) | (Allow|Warn → ToolCalled → execute_tool → ToolCompleted)
    pub fn check_guardrails(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> ironhermes_hooks::GuardrailDecision {
        // MED-01 fix: RANK pending decisions instead of last-write-wins. Precedence
        // is Block > NeedsApproval > Warn > Allow. Previously `Warn` and
        // `NeedsApproval` shared one last-write-wins cell, so a `Warn` guardrail
        // ordered AFTER a `NeedsApproval` overwrote it → the agent-loop Allow|Warn
        // arm executed the tool instead of parking it for approval (fail-OPEN
        // downgrade, T-45-05 violation). NeedsApproval must never be downgraded to
        // Warn. `Block` still wins immediately (returns on first match).
        let mut pending: Option<ironhermes_hooks::GuardrailDecision> = None;
        for guardrail in &self.guardrails {
            match guardrail.check(name, args) {
                ironhermes_hooks::GuardrailDecision::Allow => {}
                ironhermes_hooks::GuardrailDecision::Warn { reason } => {
                    tracing::warn!(
                        tool = %name,
                        guardrail = %guardrail.name(),
                        reason = %reason,
                        "Guardrail warning (proceeding)"
                    );
                    // Only record Warn if nothing stronger (NeedsApproval) is pending.
                    if !matches!(
                        pending,
                        Some(ironhermes_hooks::GuardrailDecision::NeedsApproval { .. })
                    ) {
                        pending = Some(ironhermes_hooks::GuardrailDecision::Warn { reason });
                    }
                    // Continue -- a later guardrail might Block
                }
                ironhermes_hooks::GuardrailDecision::NeedsApproval { reason } => {
                    tracing::warn!(
                        tool = %name,
                        guardrail = %guardrail.name(),
                        reason = %reason,
                        "Guardrail NeedsApproval — tool parked for chat approval (Phase 45)"
                    );
                    // NeedsApproval outranks Warn — always upgrade, never downgraded
                    // by a later Warn (that would be fail-OPEN).
                    pending = Some(ironhermes_hooks::GuardrailDecision::NeedsApproval { reason });
                    // Continue — a later guardrail might still Block
                }
                ironhermes_hooks::GuardrailDecision::Block { reason } => {
                    return ironhermes_hooks::GuardrailDecision::Block { reason };
                }
            }
        }
        pending.unwrap_or(ironhermes_hooks::GuardrailDecision::Allow)
    }

    /// Skill-as-tool fallback resolver.
    ///
    /// When `name` is not a registered tool but IS a known skill, returns the
    /// rewritten args for invoking the `skills` tool (`{action:"activate", name}`),
    /// preserving any extra args the model passed alongside the skill name. Returns
    /// `None` when there is no `SkillRegistry` handle or the name is not a skill, in
    /// which case the caller emits the normal "Tool not found" error.
    ///
    /// Skill lookup is via `SkillRegistry::find`, which is case-insensitive — so a
    /// model calling `Arxiv`/`ARXIV` resolves too. Only consulted on a tools-map
    /// miss, so a real tool sharing a skill's name always takes precedence.
    fn resolve_skill_fallback(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        let reg = self.skill_registry.as_ref()?;
        reg.find(name)?;
        let mut rewritten = serde_json::json!({ "action": "activate", "name": name });
        if let (Some(src), Some(dst)) = (args.as_object(), rewritten.as_object_mut()) {
            for (k, v) in src {
                if k != "action" && k != "name" {
                    dst.insert(k.clone(), v.clone());
                }
            }
        }
        Some(rewritten)
    }

    /// D-01 (Phase 41.3, Plan 02 Task 1): the single shared wrap around every
    /// tool-execution tail in this registry. Extracted from Plan 01's
    /// `execute_tool` wrap so `execute_tool`, its skills-rewrite fallback,
    /// `handle_tool_call`, and `dispatch_with_hook` all resolve the D-06
    /// budget and build the timeout error the same way — exactly one bounded
    /// call site and one timeout-error-string construction site in this file.
    ///
    /// `resolve_name` is the tool actually being awaited (used for the D-06
    /// `timeout_overrides` lookup and `Tool::timeout_secs()`); `display_name`
    /// is what appears in the warn log and the returned error string. These
    /// differ exactly once: the skills-rewrite fallback resolves the budget
    /// against the `skills` tool (the one really executing) but reports the
    /// model-requested alias, so an operator can correlate the timeout back
    /// to what was actually asked for.
    async fn run_bounded(
        &self,
        tool: &Arc<dyn Tool>,
        resolve_name: &str,
        display_name: &str,
        args: serde_json::Value,
    ) -> anyhow::Result<String> {
        // D-01 (Phase 41.3): bound this dispatch tail — the seam that covers
        // all five execution paths (agent loop, gateway shell.exec RPC,
        // kanban workers, delegate_task children, the realtime tool-exec
        // bridge that calls `dispatch()` directly). D-06 precedence is
        // resolved fresh on every call (never cached on the registry) so a
        // config.yaml edit takes effect without a restart.
        let tools_cfg = ironhermes_core::config::Config::load()
            .map(|c| c.tools)
            .unwrap_or_default();
        let Some(budget) = resolve_tool_timeout(tool.as_ref(), resolve_name, &tools_cfg) else {
            // Opted out (code-level `None`/`0`, not overridden) — run unbounded.
            return tool.execute(args).await;
        };

        match tokio::time::timeout(budget, tool.execute(args)).await {
            Ok(result) => result,
            Err(_elapsed) => {
                let elapsed_secs = budget.as_secs();
                // D-03: the session identifier is not in scope at this call
                // site — this signature is shared by five call paths and
                // must not change to thread one through. Empty is the
                // documented placeholder rather than a plumbing change.
                tracing::warn!(
                    tool = display_name,
                    elapsed_secs,
                    session = tracing::field::Empty,
                    "tool execution timed out"
                );
                self.timeout_counter.fetch_add(1, Ordering::Relaxed);
                // D-02: no cooperative cancellation — dropping this future
                // (the `timeout` future is dropped by `match` returning) is
                // real cancellation; every `ironhermes-exec` backend already
                // sets `kill_on_drop(true)` so OS children are reaped with no
                // new machinery.
                Err(anyhow::anyhow!(
                    "Tool '{}' timed out after {}s",
                    display_name,
                    elapsed_secs
                ))
            }
        }
    }

    /// Execute a tool by name with the given args, WITHOUT running guardrail checks.
    ///
    /// Callers MUST call `check_guardrails` first and only call this on Allow/Warn.
    /// This is the execution-only half of the D-05 split API.
    pub async fn execute_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> anyhow::Result<String> {
        let tool = match self.tools.get(name) {
            Some(t) => t,
            None => {
                // Skill-as-tool fallback: a bare skill-name call routes to `skills`.
                if let Some(rewritten) = self.resolve_skill_fallback(name, &args) {
                    tracing::info!(
                        requested = %name,
                        "routing bare skill-name call to the `skills` tool (activate)"
                    );
                    let skills = self
                        .tools
                        .get("skills")
                        .ok_or_else(|| anyhow::anyhow!("Tool not found: {}", name))?;
                    // D-01 (Phase 41.3): budget resolves against `skills` (the
                    // tool actually executing) but the warn/error reports the
                    // original requested name for operator correlation.
                    return self.run_bounded(skills, "skills", name, rewritten).await;
                }
                return Err(anyhow::anyhow!("Tool not found: {}", name));
            }
        };

        if !tool.is_available() {
            return Err(anyhow::anyhow!("Tool '{}' is not available", name));
        }

        self.run_bounded(tool, name, name, args).await
    }

    /// Invoke a tool by name, bypassing `is_available()`.
    ///
    /// This is used by integration tests that guard chromium availability themselves
    /// (D-22) and need to call tools that report `is_available() = false` because
    /// the resolver has no vision config (tests 1 and 2), or to call browser_vision
    /// with a wired resolver in test 3.
    ///
    /// Unlike `execute_tool`, this does NOT check `is_available()` — callers are
    /// responsible for ensuring the tool can run (e.g. D-22 chromium_available guard).
    pub async fn handle_tool_call(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> anyhow::Result<String> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {name}"))?;
        self.run_bounded(tool, name, name, args).await
    }

    /// Return the configured error detail level for guardrail block messages.
    /// Used by agent_loop.rs to format block errors with the same detail level
    /// as dispatch_with_hook (preserves LLM-visible error format, T-07.4-06).
    pub fn guardrail_error_detail(&self) -> &ironhermes_hooks::ErrorDetailLevel {
        &self.error_detail
    }

    /// Dispatch a tool call, optionally firing a hook after the guardrail chain permits
    /// but before the tool executes.
    ///
    /// The `post_guardrail_hook` closure is called with `(tool_name, args_str)` only when
    /// every guardrail returns Allow or Warn — never when a guardrail blocks. This ensures
    /// `ToolCalled` hook events are emitted only for permitted calls (HOOK-01 ordering fix).
    pub async fn dispatch_with_hook<F>(
        &self,
        name: &str,
        args: serde_json::Value,
        post_guardrail_hook: Option<F>,
    ) -> anyhow::Result<String>
    where
        F: FnOnce(&str, &str),
    {
        let tool = match self.tools.get(name) {
            Some(t) => t,
            None => {
                // Skill-as-tool fallback: reroute a bare skill-name call through
                // dispatch_with_hook so guardrails AND the post-guardrail hook fire
                // against the real `skills` tool. Box::pin because async-fn
                // self-recursion needs an explicit indirection.
                if let Some(rewritten) = self.resolve_skill_fallback(name, &args) {
                    tracing::info!(
                        requested = %name,
                        "routing bare skill-name call to the `skills` tool (activate)"
                    );
                    return Box::pin(self.dispatch_with_hook(
                        "skills",
                        rewritten,
                        post_guardrail_hook,
                    ))
                    .await;
                }
                return Err(anyhow::anyhow!("Tool not found: {}", name));
            }
        };

        if !tool.is_available() {
            return Err(anyhow::anyhow!("Tool '{}' is not available", name));
        }

        // Guardrail intercept (HOOK-02): check all guardrails before dispatch.
        // Per D-05: config blocklist is registered first, trait hooks second.
        // T-06-04: args reference is the same one passed to tool.execute() — no copy-after-check gap.
        for guardrail in &self.guardrails {
            match guardrail.check(name, &args) {
                ironhermes_hooks::GuardrailDecision::Allow => {}
                ironhermes_hooks::GuardrailDecision::Warn { reason } => {
                    tracing::warn!(
                        tool = %name,
                        guardrail = %guardrail.name(),
                        reason = %reason,
                        "Guardrail warning (proceeding)"
                    );
                    // Continue to next guardrail — warn does not block
                }
                ironhermes_hooks::GuardrailDecision::NeedsApproval { reason } => {
                    // dispatch_with_hook is a lower-level path; agent_loop's check_guardrails
                    // intercepts NeedsApproval before calling dispatch(). If somehow reached
                    // here, treat as fail-closed (return error) rather than proceeding.
                    let error_msg = ironhermes_hooks::format_guardrail_error(
                        name,
                        &reason,
                        guardrail.name(),
                        &self.error_detail,
                    );
                    return Err(anyhow::anyhow!("{}", error_msg));
                }
                ironhermes_hooks::GuardrailDecision::Block { reason } => {
                    let error_msg = ironhermes_hooks::format_guardrail_error(
                        name,
                        &reason,
                        guardrail.name(),
                        &self.error_detail,
                    );
                    return Err(anyhow::anyhow!("{}", error_msg));
                }
            }
        }

        // All guardrails passed — fire the post-guardrail hook before execution.
        // This is where ToolCalled events should be emitted (after permit, before execute).
        let args_str = args.to_string();
        if let Some(hook) = post_guardrail_hook {
            hook(name, &args_str);
        }

        // D-01 (Phase 41.3): the realtime tool-exec bridge's entry point —
        // `dispatch()` delegates to `dispatch_with_hook` with no tail of its
        // own, so wrapping this tail bounds `dispatch()` by inheritance.
        self.run_bounded(tool, name, name, args).await
    }

    pub fn list_tools(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.tools.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    /// D-15 (Phase 27.1.1): call `on_session_end` on every registered tool.
    /// Sync method — returns immediately. Fire-and-forget overrides (e.g.,
    /// `HexapodTcpTool::on_session_end` per D-14) internally `tokio::spawn`
    /// an async task; this method does not await those tasks.
    ///
    /// Call site contract: invoke AFTER any registry write lock is dropped
    /// (use `registry.read().await.call_session_end_hooks()`) — see Pitfall 6
    /// in 27.1.1-RESEARCH.md. CLI/TUI shutdown paths wire this in Plan 04.
    pub fn call_session_end_hooks(&self) {
        for tool in self.tools.values() {
            tool.on_session_end();
        }
    }

    /// MUST be the single source of truth for default tool registration.
    ///
    /// Do NOT hand-roll duplicate tool lists in production paths — add new tools
    /// here and they will automatically appear in every IronHermes entry point
    /// (CLI REPL, CLI batch, ratatui TUI, iron_hermes_ui, gateway).
    ///
    /// To substitute a custom terminal (e.g., one wired to a ProcessRegistry),
    /// call `register_defaults_except(&["terminal"])` then register your custom
    /// terminal variant afterwards.
    ///
    /// Current default tool set (Phase 41.3 Plan 08):
    ///   terminal, read_file, write_file, patch_file, search_files,
    ///   web_search, web_answer, web_read, hexapod_tcp, hexapod_video
    pub fn register_defaults(&mut self) {
        self.register_defaults_except(&[]);
    }

    /// Register all default tools except those whose `name()` is in `skip`.
    ///
    /// MUST be the single source of truth for default tool registration.
    /// Do NOT hand-roll duplicate tool lists in production paths.
    ///
    /// Production paths that need a process-registry-wired terminal call:
    /// ```rust,ignore
    /// registry.register_defaults_except(&["terminal"]);
    /// registry.register_terminal_tool_with_process_registry(process_registry, &config.terminal);
    /// ```
    ///
    /// **WARNING (Phase 36.3.12 GAP 1):** the bare `TerminalTool::new()` registered
    /// below for `"terminal"` carries NO `backend_config` — it always constructs a
    /// `local` backend, so `terminal.backend: docker`/`ssh` in the operator's
    /// config.yaml has zero effect on it. ANY production caller MUST skip
    /// `"terminal"` here and call `register_terminal_tool_with_process_registry`
    /// instead, or the operator's backend selection silently no-ops. Both current
    /// production composition roots (`ironhermes-agent::app_runtime_factory` and
    /// `ironhermes-cli::tui_rata::event_loop`) already do this correctly.
    pub fn register_defaults_except(&mut self, skip: &[&str]) {
        use crate::file_tools::{PatchFileTool, ReadFileTool, SearchFilesTool, WriteFileTool};
        use crate::hexapod_tcp::HexapodTcpTool;
        use crate::hexapod_video::HexapodVideoTool; // Phase 27.1.4
        use crate::terminal::TerminalTool;
        use crate::web_answer::WebAnswerTool; // Phase 41.3 Plan 08 (D-07)
        use crate::web_read::WebReadTool;
        use crate::web_search::WebSearchTool;

        macro_rules! register_unless_skipped {
            ($tool:expr, $name:expr) => {
                if !skip.contains(&$name) {
                    self.register($tool);
                }
            };
        }

        // Phase 41.3 Plan 11 (D-19): construct the credential-bearing tool
        // with this registry's resolved snapshot, not the env-only `Default`
        // — see `WebSearchTool::default()`'s doc comment for why a
        // production site using it would silently lose the config/vault tiers.
        let creds = self.credentials();

        register_unless_skipped!(Box::new(TerminalTool::new()), "terminal");
        register_unless_skipped!(Box::new(ReadFileTool), "read_file");
        register_unless_skipped!(Box::new(WriteFileTool), "write_file");
        register_unless_skipped!(Box::new(PatchFileTool), "patch"); // PatchFileTool::name() returns "patch"
        register_unless_skipped!(Box::new(SearchFilesTool), "search_files");
        register_unless_skipped!(Box::new(WebSearchTool::new(creds.clone())), "web_search");
        // Phase 41.3 Plan 08 (D-07): the answer half of the web_search /
        // web_answer split — same resolved snapshot, same "web" toolset.
        register_unless_skipped!(Box::new(WebAnswerTool::new(creds)), "web_answer");
        register_unless_skipped!(Box::new(WebReadTool), "web_read");
        // HXP-TOOL-01 (Phase 27.1.1): hexapod TCP tool — is_available() hides this when HEXAPOD_IP is unset.
        register_unless_skipped!(Box::new(HexapodTcpTool), "hexapod_tcp");
        // Phase 27.1.4: hexapod video tool — is_available() hides this when HEXAPOD_IP is unset.
        register_unless_skipped!(Box::new(HexapodVideoTool), "hexapod_video");
    }

    /// Register the memory tool with a shared `MemoryManager` handle (Plan 20-02).
    ///
    /// The handle delegates writes through the manager so the optional mirror
    /// provider is kept in sync. Callers build the handle via
    /// `ironhermes_agent::memory::factory::build_memory_manager`.
    pub fn register_memory_tool(&mut self, manager: SharedMemoryManager) {
        use crate::memory_tool::MemoryTool;
        self.register(Box::new(MemoryTool::new(manager)));
    }

    /// Register the skill_manage tool for the 'learning' toolset (Phase 33).
    ///
    /// No constructor args — `SkillManageTool` is stateless and uses
    /// `get_hermes_home()` internally for path resolution. Callers invoke
    /// this when the 'learning' toolset is enabled (Plan 33-03 wires the
    /// entry points). Not added to `register_defaults_except` — same gating
    /// pattern as `register_memory_tool` ('memory' toolset opt-in).
    pub fn register_skill_manage_tool(&mut self) {
        use crate::skill_manage::SkillManageTool;
        self.register(Box::new(SkillManageTool::new()));
    }

    /// Register the cronjob tool with a shared JobStore.
    /// Called separately from register_defaults() because it requires a JobStore instance.
    pub fn register_cronjob_tool(&mut self, store: Arc<Mutex<JobStore>>) {
        use crate::cronjob_tool::CronjobTool;
        self.register(Box::new(CronjobTool::new(store)));
    }

    /// Phase 01: register the `image_gen` text-to-image LLM tool.
    ///
    /// Called separately from `register_defaults_except` (like `register_tts_tools`)
    /// because `ImageGenTool` needs `Arc<Config>` for `image_gen.default_model` /
    /// `image_gen.timeout_secs` / `image_gen.session_cap`, which is not in scope
    /// inside `register_defaults_except`. This is the **single** registration site
    /// for `image_gen`.
    ///
    /// Plan 03 (SAFE-05 / D-03) makes this per-session: the tool is constructed
    /// with the chat's `SessionKey` and the registry's shared
    /// [`SessionCounter`](crate::image_gen::SessionCounter), so the generation
    /// cap is scoped per session. `session_key` is `None` at global / startup
    /// construction (no concrete session yet) — in that case the tool is
    /// registered without a cap (the cap binds once a real session registers).
    /// `ack_sink` surfaces the interim "generating…" notice (D-04) and may be
    /// `None` for the local/CLI path.
    ///
    /// The tool's `is_available()` hides it from the LLM schema when `FAL_KEY` is
    /// unset (GEN-03).
    ///
    /// Phase 47 Plan 08 (GEN-05): `guardrail` is `Some((guardrail, kind))` to
    /// thread the shared [`crate::gen_guardrail::GenerationGuardrail`]
    /// chokepoint into the constructed tool (the chat surface passes `Root`;
    /// a kanban-worker re-registration passes `Descendant`). `None` keeps
    /// pre-Plan-08 callers/tests compiling unchanged (no guardrail enforced —
    /// only the existing per-session cap applies).
    pub fn register_image_gen_tool(
        &mut self,
        config: Arc<ironhermes_core::Config>,
        session_key: Option<ironhermes_core::SessionKey>,
        ack_sink: Option<Arc<dyn crate::image_gen::AckSink>>,
        guardrail: Option<(
            Arc<crate::gen_guardrail::GenerationGuardrail>,
            crate::gen_guardrail::ReservationKind,
        )>,
    ) {
        use crate::fal::FalClient;
        use crate::image_gen::ImageGenTool;
        let mut tool = match session_key {
            Some(key) => ImageGenTool::new_with_session(
                key,
                config,
                FalClient::new(),
                self.image_gen_counter.clone(),
                ack_sink,
            ),
            None => ImageGenTool::new(config, FalClient::new()),
        };
        if let Some((g, kind)) = guardrail {
            tool = tool.with_guardrail(g, kind);
        }
        self.register(Box::new(tool));
    }

    /// Phase 36.3.3 Plan 02: register `video_gen` (T2V) and `video_animate` (I2V) LLM tools.
    ///
    /// Called per-session so each `VideoGenerateTool` / `VideoAnimateTool` instance
    /// carries the chat's [`SessionKey`] and shares the registry's
    /// [`VideoSessionCounter`](crate::video_gen::VideoSessionCounter). The cap is
    /// scoped per `SessionKey` — one session hitting the limit never blocks another.
    ///
    /// `session_key: None` at global / startup construction registers stateless tools
    /// (no cap enforced). The tools' `is_available()` hides them from the LLM schema
    /// when `FAL_KEY` is unset (D-12).
    ///
    /// Phase 47 Plan 08 (GEN-05): `guardrail` is threaded into BOTH t2v/i2v
    /// tool instances (same shape/rationale as `register_image_gen_tool`).
    pub fn register_video_gen_tools(
        &mut self,
        config: Arc<ironhermes_core::Config>,
        session_key: Option<ironhermes_core::SessionKey>,
        ack_sink: Option<Arc<dyn crate::image_gen::AckSink>>,
        guardrail: Option<(
            Arc<crate::gen_guardrail::GenerationGuardrail>,
            crate::gen_guardrail::ReservationKind,
        )>,
    ) {
        use crate::fal::FalClient;
        use crate::video_gen::{VideoAnimateTool, VideoGenerateTool};

        let (mut t2v, mut i2v) = match session_key {
            Some(key) => (
                VideoGenerateTool::new_with_session(
                    key.clone(),
                    config.clone(),
                    FalClient::new(),
                    self.video_gen_counter.clone(),
                    ack_sink.clone(),
                ),
                VideoAnimateTool::new_with_session(
                    key,
                    config,
                    FalClient::new(),
                    self.video_gen_counter.clone(),
                    ack_sink,
                ),
            ),
            None => (
                VideoGenerateTool::new(config.clone(), FalClient::new()),
                VideoAnimateTool::new(config, FalClient::new()),
            ),
        };
        if let Some((g, kind)) = guardrail {
            t2v = t2v.with_guardrail(g.clone(), kind.clone());
            i2v = i2v.with_guardrail(g, kind);
        }
        self.register(Box::new(t2v));
        self.register(Box::new(i2v));
    }

    /// Phase 47 Plan 08: register the `video_to_video` (v2v) LLM tool —
    /// mirrors `register_video_gen_tools` exactly (same per-session /
    /// guardrail shape), the missing sibling registration this plan adds so
    /// v2v is registered wherever the sibling video tools are.
    pub fn register_video_to_video_tool(
        &mut self,
        config: Arc<ironhermes_core::Config>,
        session_key: Option<ironhermes_core::SessionKey>,
        ack_sink: Option<Arc<dyn crate::image_gen::AckSink>>,
        guardrail: Option<(
            Arc<crate::gen_guardrail::GenerationGuardrail>,
            crate::gen_guardrail::ReservationKind,
        )>,
    ) {
        use crate::fal::FalClient;
        use crate::video_to_video::VideoToVideoTool;

        let mut tool = match session_key {
            Some(key) => VideoToVideoTool::new_with_session(
                key,
                config,
                FalClient::new(),
                self.video_gen_counter.clone(),
                ack_sink,
            ),
            None => VideoToVideoTool::new(config, FalClient::new()),
        };
        if let Some((g, kind)) = guardrail {
            tool = tool.with_guardrail(g, kind);
        }
        self.register(Box::new(tool));
    }

    /// Phase 36.17.5: register text_to_speech + send_audio LLM tools.
    ///
    /// Called per-session because SendAudioTool's SessionKey + Telegram dispatcher
    /// are injected at construction time (D-14 / D-15). The TextToSpeechTool itself
    /// is session-independent but registered alongside for symmetry.
    ///
    /// `dispatcher: None` is correct for Platform::Local / CLI paths; the Local arm
    /// in SendAudioTool::execute plays locally via rodio without needing a dispatcher.
    pub fn register_tts_tools(
        &mut self,
        session_key: ironhermes_core::SessionKey,
        dispatcher: Option<Arc<dyn crate::AudioDispatcher>>,
        config: Arc<ironhermes_core::Config>,
    ) {
        use crate::send_audio_tool::SendAudioTool;
        use crate::tts::build_tts_registry;
        use crate::tts_tool::TextToSpeechTool;

        let tts_registry = Arc::new(build_tts_registry(&config.tts));
        // Phase 36.17.7 D-06 (Path B): record the session_key on the registry
        // BEFORE SendAudioTool consumes it via move — so
        // `tts_registration_status()` can later distinguish Live vs Inspection.
        self.tts_session_key = Some(session_key.clone());
        self.register(Box::new(TextToSpeechTool::new(
            config.clone(),
            tts_registry,
        )));
        self.register(Box::new(SendAudioTool::new(
            session_key,
            dispatcher,
            config,
        )));
    }

    /// Phase 36.3.8 D-02/D-04/D-05: register send_message + clarify LLM tools per turn.
    ///
    /// Mirrors `register_tts_tools` exactly: both tools need constructor-injected
    /// `SessionKey` + optional dispatchers at registration time. `ToolRegistry::register`
    /// uses `HashMap::insert` (upsert by name) so repeated turns idempotently replace the
    /// prior instance — same property `register_tts_tools` relies on.
    ///
    /// `message_dispatcher: None` is correct for Platform::Local (stdout path in tool).
    /// `clarify_dispatcher: None` drives the stdout numbered fallback in ClarifyTool.
    /// `clarify_registry` MUST be the same Arc shared with the gateway callback loop
    /// so a button tap resolves the correct awaiter (T-36.3.8-ROUTE).
    pub fn register_messaging_tools(
        &mut self,
        session_key: ironhermes_core::SessionKey,
        message_dispatcher: Option<Arc<dyn crate::MessageDispatcher>>,
        clarify_dispatcher: Option<Arc<dyn crate::ClarifyDispatcher>>,
        clarify_registry: Arc<crate::clarify_registry::PendingClarifyRegistry>,
        cancel_token: Option<tokio_util::sync::CancellationToken>,
        config: Arc<ironhermes_core::Config>,
    ) {
        use crate::clarify_tool::ClarifyTool;
        use crate::send_message_tool::SendMessageTool;

        self.register(Box::new(SendMessageTool::new(
            session_key.clone(),
            message_dispatcher,
            config.clone(),
        )));
        self.register(Box::new(ClarifyTool::new(
            session_key,
            clarify_dispatcher,
            clarify_registry,
            cancel_token,
            config,
        )));
    }

    /// Phase 36.17.8: build and attach the STT registry alongside the TTS registry.
    ///
    /// Called once at startup (CLI capture loop, Plan 05) or per-session (Plan 06 web path).
    /// The resulting `Arc<SttRegistry>` is stored on this registry and exposed via
    /// `self.stt_registry` so the capture loop and web server share the same instance.
    ///
    /// NOT an LLM tool — STT is a capture-loop feature, not Claude-accessible (RESEARCH
    /// Claude's-Discretion). Do NOT call `self.register(...)` here.
    pub fn register_stt_registry(&mut self, config: Arc<ironhermes_core::Config>) {
        use crate::stt::build_stt_registry;
        self.stt_registry = Some(Arc::new(build_stt_registry(&config.stt)));
    }

    /// Phase 25.1 D-04: register all 11 browser_* tools sharing one Arc<Mutex<Option<BrowserSession>>>.
    ///
    /// Mirrors register_memory_tool / register_cronjob_tool wiring shape.
    /// Caller pre-constructs the Arc so AgentLoop can hold the same instance via with_browser_session.
    /// browser_vision additionally takes the resolver Arc for D-06 vision capability checks.
    pub fn register_browser_tools(
        &mut self,
        session: std::sync::Arc<tokio::sync::Mutex<Option<crate::browser_session::BrowserSession>>>,
        resolver: std::sync::Arc<ironhermes_core::provider::ProviderResolver>,
        config: std::sync::Arc<ironhermes_core::config::Config>,
    ) {
        use crate::browser_back::BrowserBackTool;
        use crate::browser_click::BrowserClickTool;
        use crate::browser_close::BrowserCloseTool;
        use crate::browser_console::BrowserConsoleTool;
        use crate::browser_get_images::BrowserGetImagesTool;
        use crate::browser_navigate::BrowserNavigateTool;
        use crate::browser_press::BrowserPressTool;
        use crate::browser_scroll::BrowserScrollTool;
        use crate::browser_snapshot::BrowserSnapshotTool;
        use crate::browser_type::BrowserTypeTool;
        use crate::browser_vision::BrowserVisionTool;

        // VisionClientHandle stub: a no-op handle used when the real agent-side impl
        // is not available (e.g. in unit tests). The real impl is wired in AgentLoop
        // via plan 09's AnyClientVisionHandle.
        let vision_client = std::sync::Arc::new(crate::browser_vision::NoOpVisionHandle);

        self.register(Box::new(BrowserBackTool::new(session.clone())));
        self.register(Box::new(BrowserClickTool::new(session.clone())));
        self.register(Box::new(BrowserCloseTool::new(session.clone())));
        self.register(Box::new(BrowserConsoleTool::new(
            session.clone(),
            config.clone(),
        )));
        self.register(Box::new(BrowserGetImagesTool::new(session.clone())));
        self.register(Box::new(BrowserNavigateTool::new(
            session.clone(),
            config.clone(),
        )));
        self.register(Box::new(BrowserPressTool::new(session.clone())));
        self.register(Box::new(BrowserScrollTool::new(session.clone())));
        self.register(Box::new(BrowserSnapshotTool::new(session.clone())));
        self.register(Box::new(BrowserTypeTool::new(session.clone())));
        self.register(Box::new(BrowserVisionTool::new(
            session.clone(),
            resolver,
            vision_client,
        )));
    }

    /// Phase 25.1 D-07 / OQ-5: register all 11 browser_* tools with a real VisionClientHandle.
    ///
    /// Variant of `register_browser_tools` used by production CLI entry points after
    /// `AnyClientVisionHandle` is available (plan 11 wiring). The real handle routes
    /// `browser_vision` calls through the Phase 26 D-07 cascade rather than the NoOp stub.
    pub fn register_browser_tools_with_vision(
        &mut self,
        session: std::sync::Arc<tokio::sync::Mutex<Option<crate::browser_session::BrowserSession>>>,
        resolver: std::sync::Arc<ironhermes_core::provider::ProviderResolver>,
        vision_client: std::sync::Arc<dyn crate::browser_vision::VisionClientHandle>,
        config: std::sync::Arc<ironhermes_core::config::Config>,
    ) {
        use crate::browser_back::BrowserBackTool;
        use crate::browser_click::BrowserClickTool;
        use crate::browser_close::BrowserCloseTool;
        use crate::browser_console::BrowserConsoleTool;
        use crate::browser_get_images::BrowserGetImagesTool;
        use crate::browser_navigate::BrowserNavigateTool;
        use crate::browser_press::BrowserPressTool;
        use crate::browser_scroll::BrowserScrollTool;
        use crate::browser_snapshot::BrowserSnapshotTool;
        use crate::browser_type::BrowserTypeTool;
        use crate::browser_vision::BrowserVisionTool;

        self.register(Box::new(BrowserBackTool::new(session.clone())));
        self.register(Box::new(BrowserClickTool::new(session.clone())));
        self.register(Box::new(BrowserCloseTool::new(session.clone())));
        self.register(Box::new(BrowserConsoleTool::new(
            session.clone(),
            config.clone(),
        )));
        self.register(Box::new(BrowserGetImagesTool::new(session.clone())));
        self.register(Box::new(BrowserNavigateTool::new(
            session.clone(),
            config.clone(),
        )));
        self.register(Box::new(BrowserPressTool::new(session.clone())));
        self.register(Box::new(BrowserScrollTool::new(session.clone())));
        self.register(Box::new(BrowserSnapshotTool::new(session.clone())));
        self.register(Box::new(BrowserTypeTool::new(session.clone())));
        self.register(Box::new(BrowserVisionTool::new(
            session.clone(),
            resolver,
            vision_client,
        )));
    }

    /// Phase 25.2 D-13: register `web_extract` tool with injected summarization client handle
    /// and skill registry. Called from agent crate's AgentLoop init AFTER `register_defaults()`,
    /// because WebExtractTool needs both handles wired before it can dispatch summarization
    /// or YouTube skill calls. Separate from register_defaults() because the constructor takes
    /// runtime-built handles that the registry crate cannot construct itself.
    pub fn register_web_extract_tool(
        &mut self,
        summarization_client: std::sync::Arc<dyn ironhermes_core::SummarizationClientHandle>,
        skill_registry: std::sync::Arc<ironhermes_core::SkillRegistry>,
    ) {
        use crate::web_extract::WebExtractTool;
        self.register(Box::new(WebExtractTool::new(
            summarization_client,
            skill_registry,
        )));
    }

    /// Register the skills tool with a shared SkillRegistry and active_skills tracker.
    /// Called separately from register_defaults() because it requires a SkillRegistry instance.
    ///
    /// Phase 19 Plan 03: now also takes `credential_dir` (root for per-skill credentials,
    /// per D-10) and `skills_config` (per-skill config map reserved for Plan 04 injection).
    pub fn register_skills_tool(
        &mut self,
        registry: Arc<ironhermes_core::SkillRegistry>,
        active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
        credential_dir: std::path::PathBuf,
        skills_config: std::collections::HashMap<
            String,
            std::collections::HashMap<String, serde_yaml::Value>,
        >,
    ) {
        use crate::skills_tool::SkillsTool;
        // Keep a handle for the skill-as-tool dispatch fallback (see execute_tool /
        // dispatch_with_hook). Cloned Arc — same registry the SkillsTool reads.
        self.skill_registry = Some(registry.clone());
        self.register(Box::new(SkillsTool::new(
            registry,
            active_skills,
            credential_dir,
            skills_config,
        )));
    }

    /// Register the delegate_task tool with a SubagentRunner, semaphore, and config.
    ///
    /// The `runner` implements the `SubagentRunner` trait (defined in delegate_task.rs)
    /// and is typically constructed in ironhermes-agent to wrap AgentLoop::run().
    ///
    /// Phase 47 Plan 08 (D-09/D-10): `generation_wiring` is
    /// `Some(GenerationSurfaceWiring)` ONLY when `guardrails.surfaces.delegate`
    /// is true — its presence is exactly what makes the `"generation"`
    /// delegate toolset group non-empty (fail-closed: `None` here means a
    /// child's `toolsets: ["generation"]` resolves to an EMPTY tool list, see
    /// `delegate_task::resolve_toolset_tools`).
    #[allow(clippy::too_many_arguments)]
    pub fn register_delegate_task_tool(
        &mut self,
        runner: Arc<dyn crate::delegate_task::SubagentRunner>,
        semaphore: Arc<tokio::sync::Semaphore>,
        memory_manager: Option<SharedMemoryManager>,
        config: ironhermes_core::SubagentConfig,
        cancel_token: Option<tokio_util::sync::CancellationToken>,
        progress_callback: Option<crate::delegate_task::SubagentProgressCallback>,
        generation_wiring: Option<crate::delegate_task::GenerationSurfaceWiring>,
    ) {
        use crate::delegate_task::DelegateTaskTool;
        let mut tool = DelegateTaskTool::new(runner, semaphore, memory_manager, config, cancel_token)
            // Phase 41.3 Plan 11 (D-19): always install this registry's own
            // resolved snapshot — a delegate child must never silently fall
            // back to `DelegateTaskTool::new`'s env-only default.
            .with_credentials(self.credentials());
        if let Some(cb) = progress_callback {
            tool = tool.with_progress_callback(cb);
        }
        if let Some(w) = generation_wiring {
            tool = tool.with_generation_wiring(w);
        }
        self.register(Box::new(tool));
    }

    /// Register the `artifact` tool (Phase 46.6 D-01/D-03/D-05/D-06).
    ///
    /// Builds the tool's `AuditLog` from `config.audit` (mirrors
    /// `AuditLog::load(config.audit.clone())` at `ironhermes-gateway/src/runner.rs:754`)
    /// and pins the per-run/per-task cap to [`crate::artifact::DEFAULT_ARTIFACT_CAP`].
    /// Called at each producer surface (chat/delegate/kanban — Plan 04 wiring); the
    /// tool's `toolset()` is `"artifacts"`, a normal `ALL_TOOLSETS` member (not the
    /// MCP/KANBAN exemption path — RESEARCH Pitfall 4), so it needs no special
    /// per-surface config plumbing beyond a normal `register()` call.
    pub fn register_artifact_tool(&mut self, config: Arc<ironhermes_core::Config>) {
        use crate::artifact::{ArtifactTool, DEFAULT_ARTIFACT_CAP};
        let audit = Arc::new(ironhermes_core::AuditLog::load(config.audit.clone()));
        self.register(Box::new(ArtifactTool::new(audit, DEFAULT_ARTIFACT_CAP)));
    }

    /// Register the execute_code tool with a separate RPC dispatch registry.
    ///
    /// `rpc_registry` must contain ONLY D-07 safe tools (no terminal, no execute_code).
    /// This is built separately from the main registry to structurally prevent recursion
    /// and terminal access from sandboxed scripts.
    ///
    /// Called AFTER all other tools are registered but BEFORE wrapping in Arc.
    pub fn register_execute_code_tool(
        &mut self,
        rpc_registry: Arc<ToolRegistry>,
        config: ironhermes_core::ExecConfig,
    ) {
        use crate::execute_code::ExecuteCodeTool;
        self.register(Box::new(ExecuteCodeTool::new(rpc_registry, config, None)));
    }

    /// Phase 19 Plan 06 (D-05): register execute_code with shared access to the
    /// active-skills list so skill-declared env vars bypass the sandbox secret-strip.
    pub fn register_execute_code_tool_with_active_skills(
        &mut self,
        rpc_registry: Arc<ToolRegistry>,
        config: ironhermes_core::ExecConfig,
        active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
    ) {
        use crate::execute_code::ExecuteCodeTool;
        self.register(Box::new(ExecuteCodeTool::with_active_skills(
            rpc_registry,
            config,
            None,
            active_skills,
        )));
    }

    /// Phase 21.7-06 (D-29): register execute_code with BOTH active-skills
    /// bypass AND a shared `ProcessRegistry` handle for the `background=true`
    /// branch. Replaces the `_with_active_skills` registration at the three
    /// CLI + gateway call sites so INV-21.7-03 totals 3 new + 0 legacy after
    /// Plan 06 wiring lands. Foreground (sandbox) mode is unchanged.
    pub fn register_execute_code_tool_with_process_registry(
        &mut self,
        rpc_registry: Arc<ToolRegistry>,
        config: ironhermes_core::ExecConfig,
        active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
        process_registry: Arc<
            tokio::sync::RwLock<ironhermes_exec::process_registry::ProcessRegistry>,
        >,
    ) {
        use crate::execute_code::ExecuteCodeTool;
        let tool = ExecuteCodeTool::with_active_skills(rpc_registry, config, None, active_skills)
            .with_process_registry(process_registry);
        self.register(Box::new(tool));
    }

    /// Phase 21.7-06 (D-29): register a `TerminalTool` whose `background=true`
    /// branch is wired to the session-scoped `ProcessRegistry`. Foreground
    /// behaviour is unchanged. Called from the three CLI sites + gateway
    /// runner when background spawning is desired.
    ///
    /// Phase 42 EXEC-03 / D-05: threads `TerminalConfig.terminal_env_allowlist`
    /// from the caller's `Config` so operator-opted-in vars reach the child subprocess.
    ///
    /// Phase 36.3.12 GAP 1 (D-01/D-06/D-07/D-09): this is the ONLY production path
    /// that makes `terminal.backend` (docker/ssh/local), `container_runtime`, and
    /// `forward_env` take effect — it threads the operator's full resolved
    /// `TerminalConfig` into the registered tool via `with_backend_config`. Before
    /// this wiring, every production caller built a bare `TerminalTool::new()`
    /// (`backend_config: None`), so `terminal.backend: docker`/`ssh` in config.yaml
    /// was a silent no-op that always ran the command on the host. See
    /// `crates/ironhermes-tools/tests/terminal_backend_selection_wiring.rs`.
    pub fn register_terminal_tool_with_process_registry(
        &mut self,
        process_registry: Arc<
            tokio::sync::RwLock<ironhermes_exec::process_registry::ProcessRegistry>,
        >,
        terminal_config: &ironhermes_core::config::TerminalConfig,
    ) {
        use crate::terminal::TerminalTool;
        let tool = TerminalTool::new()
            .with_env_allowlist(terminal_config.terminal_env_allowlist.clone())
            .with_process_registry(process_registry)
            .with_backend_config(terminal_config.clone());
        self.register(Box::new(tool));
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Phase 25 D-23: intercepted_owner_toolset helper
// Maps intercepted tool names to their owning toolset for D-23 layer-1 filtering.
// ---------------------------------------------------------------------------

/// Maps an intercepted tool name to its owning toolset (D-13 / D-23 Plan 3).
/// Used by `get_definitions()` to apply toolset-level filtering to intercepts.
/// Default toolset is "agent" for any unknown intercepted name.
fn intercepted_owner_toolset(name: &str) -> &'static str {
    match name {
        "memory" => "memory",
        "session_search" => "session",
        "delegate_task" | "todo_write" | "todo_read" | "cronjob" => "agent",
        _ => "agent",
    }
}

// ---------------------------------------------------------------------------
// Phase 25 D-13 / Open Question 2: greenfield todo_* schema constructors.
// These are free functions (not Tool impls) because the in-session state
// (Arc<Mutex<Vec<String>>>) is owned by AgentLoop, not a Tool struct.
// Plan 3 wires real handlers via AgentLoop::with_intercepts(). Plan 2 ships
// only the schema constructors — do NOT register them in ToolRegistry::new().
// ---------------------------------------------------------------------------

/// Phase 25 D-13 / Open Question 2 (Plan 2): minimal greenfield schema for the
/// intercepted `todo_write` tool. `items` replaces the current todo list.
/// In-session state lives in `Arc<Mutex<Vec<String>>>` owned by AgentLoop and
/// passed to `with_intercepts()` (D-16, wired in Plan 3).
pub fn todo_write_schema() -> ToolSchema {
    ToolSchema::new(
        "todo_write",
        "Write (replace) the current todo list for this session. Replaces the entire list with the provided items.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "New todo list items. Replaces the entire current list."
                }
            },
            "required": ["items"]
        }),
    )
}

/// Phase 25 D-13 / Open Question 2 (Plan 2): minimal greenfield schema for
/// `todo_read`. Returns the current list. No required parameters.
pub fn todo_read_schema() -> ToolSchema {
    ToolSchema::new(
        "todo_read",
        "Read the current todo list for this session.",
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ironhermes_core::ToolSchema;
    use ironhermes_hooks::{BlocklistGuardrail, GuardrailDecision, GuardrailHook};
    use std::sync::{Mutex, OnceLock};

    // ---------------------------------------------------------------------------
    // env_lock: serialise tests that mutate environment variables.
    // Copied from crates/ironhermes-cli/tests/profile_isolation.rs pattern.
    // Phase 21.6 D: Rust 2024 edition requires unsafe for env var mutation.
    // ---------------------------------------------------------------------------

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// Phase 25.2 Plan 12 Task 2: lock the public signature of register_web_extract_tool
    /// against drift. Plan 14 wires this method against AnyClientSummarizationHandle from
    /// the agent crate; if the signature shape ever changes, this test fails at compile time.
    #[test]
    fn register_web_extract_tool_signature_locked() {
        let _: fn(
            &mut ToolRegistry,
            std::sync::Arc<dyn ironhermes_core::SummarizationClientHandle>,
            std::sync::Arc<ironhermes_core::SkillRegistry>,
        ) = ToolRegistry::register_web_extract_tool;
    }

    // ---------------------------------------------------------------------------
    // Mock tool for testing
    // ---------------------------------------------------------------------------

    struct MockTool {
        tool_name: &'static str,
    }

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            self.tool_name
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "mock tool for testing"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                self.tool_name,
                self.description(),
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("mock result".to_string())
        }
    }

    // ---------------------------------------------------------------------------
    // Warn-only guardrail for testing
    // ---------------------------------------------------------------------------

    struct WarnGuardrail;

    impl GuardrailHook for WarnGuardrail {
        fn check(&self, _tool_name: &str, _args: &serde_json::Value) -> GuardrailDecision {
            GuardrailDecision::Warn {
                reason: "always warn".to_string(),
            }
        }
        fn name(&self) -> &str {
            "warn-always"
        }
    }

    /// MED-01: NeedsApproval-only guardrail for precedence testing.
    struct NeedsApprovalGuardrail;

    impl GuardrailHook for NeedsApprovalGuardrail {
        fn check(&self, _tool_name: &str, _args: &serde_json::Value) -> GuardrailDecision {
            GuardrailDecision::NeedsApproval {
                reason: "needs approval".to_string(),
            }
        }
        fn name(&self) -> &str {
            "needs-approval-always"
        }
    }

    /// MED-01 regression: an ordered `[NeedsApproval, Warn]` guardrail chain must
    /// yield `NeedsApproval`. Previously `check_guardrails` used a single
    /// last-write-wins `last_warn` cell, so a `Warn` ordered AFTER a `NeedsApproval`
    /// overwrote it → the tool executed instead of parking for approval (fail-OPEN
    /// downgrade, T-45-05). The fix ranks Block > NeedsApproval > Warn > Allow.
    #[test]
    fn check_guardrails_ranks_needsapproval_over_later_warn() {
        let mut registry = make_registry_with_tool("srv__delete");
        // Order matters: NeedsApproval FIRST, Warn SECOND (the fail-OPEN trigger).
        registry.add_guardrail(Box::new(NeedsApprovalGuardrail));
        registry.add_guardrail(Box::new(WarnGuardrail));
        let decision = registry.check_guardrails("srv__delete", &serde_json::json!({}));
        assert!(
            matches!(decision, GuardrailDecision::NeedsApproval { .. }),
            "MED-01: a later Warn must NOT downgrade an earlier NeedsApproval; got {decision:?}"
        );
    }

    /// MED-01 companion: `Block` still wins over a pending `NeedsApproval`
    /// (precedence Block > NeedsApproval), verifying the ranking did not weaken the
    /// existing Block-first invariant.
    #[test]
    fn check_guardrails_block_beats_needsapproval() {
        let mut registry = make_registry_with_tool("srv__delete");
        registry.add_guardrail(Box::new(NeedsApprovalGuardrail));
        registry.add_guardrail(Box::new(BlocklistGuardrail::new(vec![
            "srv__delete".to_string(),
        ])));
        let decision = registry.check_guardrails("srv__delete", &serde_json::json!({}));
        assert!(
            matches!(decision, GuardrailDecision::Block { .. }),
            "MED-01: Block must win over NeedsApproval; got {decision:?}"
        );
    }

    fn make_registry_with_tool(tool_name: &'static str) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MockTool { tool_name }));
        registry
    }

    // ---------------------------------------------------------------------------
    // Test tools for prerequisite tests
    // ---------------------------------------------------------------------------

    /// Tool with no prerequisites() override — uses the default empty Vec.
    struct NoPrereqTool;

    #[async_trait]
    impl Tool for NoPrereqTool {
        fn name(&self) -> &str {
            "no_prereq"
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "tool with no prerequisites"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "no_prereq",
                "tool with no prerequisites",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
        // prerequisites() intentionally NOT overridden — tests the default
    }

    /// Tool with one required env_var prerequisite.
    struct RequiredEnvPrereqTool;

    #[async_trait]
    impl Tool for RequiredEnvPrereqTool {
        fn name(&self) -> &str {
            "required_env_prereq"
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "tool with required env_var prerequisite"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "required_env_prereq",
                "tool with required env_var prerequisite",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
        fn prerequisites(&self) -> Vec<Prerequisite> {
            vec![Prerequisite {
                kind: "env_var".to_string(),
                name: "TEST_PREREQ_25_01_PRESENT".to_string(),
                description: "Test prerequisite env var for Phase 25 Plan 01 unit tests."
                    .to_string(),
                required: true,
                group: None,
            }]
        }
    }

    /// Tool with one optional (required:false) env_var prerequisite.
    struct OptionalEnvPrereqTool;

    #[async_trait]
    impl Tool for OptionalEnvPrereqTool {
        fn name(&self) -> &str {
            "optional_env_prereq"
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "tool with optional env_var prerequisite"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "optional_env_prereq",
                "tool with optional env_var prerequisite",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
        fn prerequisites(&self) -> Vec<Prerequisite> {
            vec![Prerequisite {
                kind: "env_var".to_string(),
                name: "TEST_PREREQ_25_01_PRESENT".to_string(),
                description: "Optional test prerequisite env var.".to_string(),
                required: false,
                group: None,
            }]
        }
    }

    /// Tool with a config_field prerequisite and no `credentials()` snapshot — under
    /// D-18 (Phase 41.3 Plan 10) this now gates `is_available()` CLOSED (the
    /// reversal of the pre-Plan-10 unconditional-true default). See
    /// `config_field_prereq_without_a_credential_snapshot_gates_closed` and
    /// `config_field_prereq_is_satisfied_when_the_snapshot_has_the_path` for the
    /// closed/open pair this fixture participates in.
    struct ConfigFieldPrereqTool;

    #[async_trait]
    impl Tool for ConfigFieldPrereqTool {
        fn name(&self) -> &str {
            "config_field_prereq"
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "tool with config_field prerequisite"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "config_field_prereq",
                "tool with config_field prerequisite",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
        fn prerequisites(&self) -> Vec<Prerequisite> {
            vec![Prerequisite {
                kind: "config_field".to_string(),
                name: "search.brave_api_key".to_string(),
                description: "Config field prerequisite — checked at config load, not trait level."
                    .to_string(),
                required: true,
                group: None,
            }]
        }
    }

    // ---------------------------------------------------------------------------
    // Phase 25 Plan 01: Prerequisite struct + default is_available() tests
    // ---------------------------------------------------------------------------

    /// Test 1: A struct implementing Tool with no prerequisites() override returns
    /// empty Vec from prerequisites().
    #[test]
    fn prerequisite_default_impl_returns_empty() {
        let tool = NoPrereqTool;
        let prereqs = tool.prerequisites();
        assert!(
            prereqs.is_empty(),
            "default prerequisites() must return empty Vec"
        );
    }

    /// Test 2: A test Tool whose prerequisites() returns one required env_var prereq,
    /// when the env var IS set, has is_available() == true.
    #[test]
    fn is_available_default_walks_prerequisites_required_env_var_present() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: single-threaded test with env_lock held; Rust 2024 edition requires unsafe.
        unsafe { std::env::set_var("TEST_PREREQ_25_01_PRESENT", "1") };
        let tool = RequiredEnvPrereqTool;
        let available = tool.is_available();
        unsafe { std::env::remove_var("TEST_PREREQ_25_01_PRESENT") };
        assert!(
            available,
            "is_available() must be true when required env_var prereq is set"
        );
    }

    /// Test 3: Same Tool when the env var is NOT set has is_available() == false.
    #[test]
    fn is_available_default_walks_prerequisites_required_env_var_absent() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: single-threaded test with env_lock held; Rust 2024 edition requires unsafe.
        unsafe { std::env::remove_var("TEST_PREREQ_25_01_PRESENT") };
        let tool = RequiredEnvPrereqTool;
        let available = tool.is_available();
        assert!(
            !available,
            "is_available() must be false when required env_var prereq is absent"
        );
    }

    /// Test 4: A test Tool with required:false for an unset env var has is_available() == true
    /// (optional prereqs do not block).
    #[test]
    fn is_available_default_walks_prerequisites_optional_env_var_absent() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: single-threaded test with env_lock held; Rust 2024 edition requires unsafe.
        unsafe { std::env::remove_var("TEST_PREREQ_25_01_PRESENT") };
        let tool = OptionalEnvPrereqTool;
        let available = tool.is_available();
        assert!(
            available,
            "is_available() must be true when only optional prereqs are absent"
        );
    }

    // ---------------------------------------------------------------------------
    // D-18 (Phase 41.3 Plan 10): the rewrite of the old "Test 5" (the pre-Plan-10
    // config_field-arm test, formerly named for asserting an unrecognised kind is
    // satisfied). That test bundled two claims — "config_field is non-blocking"
    // (D-18 reverses this) and "an unrecognised kind is non-blocking" (D-25
    // preserves this) — split below into one case each, per the plan's
    // instruction to split rather than delete.
    // ---------------------------------------------------------------------------

    /// D-18: a `config_field` prerequisite with no `Tool::credentials()` snapshot
    /// gates `is_available()` CLOSED. This reverses the pre-Plan-10 default (which
    /// treated `config_field` as unconditionally satisfied) — see
    /// `config_field_prereq_is_satisfied_when_the_snapshot_has_the_path` for the
    /// matching "gates open when the snapshot has the path" case.
    #[test]
    fn config_field_prereq_without_a_credential_snapshot_gates_closed() {
        let tool = ConfigFieldPrereqTool;
        let available = tool.is_available();
        assert!(
            !available,
            "is_available() must be false for a config_field prereq when the tool \
             carries no credential snapshot (D-18)"
        );
    }

    /// Tool with a prerequisite of an unrecognised kind (neither "env_var" nor
    /// "config_field") — probes the D-25 catch-all arm directly.
    struct UnknownKindPrereqTool;

    #[async_trait]
    impl Tool for UnknownKindPrereqTool {
        fn name(&self) -> &str {
            "unknown_kind_prereq"
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "tool with an unrecognised prerequisite kind"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "unknown_kind_prereq",
                "tool with an unrecognised prerequisite kind",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
        fn prerequisites(&self) -> Vec<Prerequisite> {
            vec![Prerequisite {
                kind: "totally_unrecognized_probe_kind".to_string(),
                name: "some-probe".to_string(),
                description: "unrecognised prereq kind — non-blocking per D-25".to_string(),
                required: true,
                group: None,
            }]
        }
    }

    /// D-25: an unrecognised prerequisite `kind` remains non-blocking — the half of
    /// the old bundled test D-18 does NOT reverse.
    #[test]
    fn unknown_prereq_kind_is_still_non_blocking() {
        let available = UnknownKindPrereqTool.is_available();
        assert!(
            available,
            "an unrecognised prerequisite kind must remain non-blocking (D-25)"
        );
    }

    // ---------------------------------------------------------------------------
    // Phase 48.2 Plan 10 (D-16/G-48.2-3): `Prerequisite::runtime` + the "runtime"
    // satisfaction arm.
    // ---------------------------------------------------------------------------

    /// `Prerequisite::runtime` sets `kind: "runtime"`, `required: true`,
    /// `group: None`, and carries the given name/description through unchanged.
    #[test]
    fn prerequisite_runtime_constructor_field_values() {
        let p = Prerequisite::runtime("clarify_channel", "no clarification channel available");
        assert_eq!(p.kind, "runtime");
        assert_eq!(p.name, "clarify_channel");
        assert_eq!(p.description, "no clarification channel available");
        assert!(p.required, "a runtime prerequisite is always required");
        assert!(
            p.group.is_none(),
            "a runtime prerequisite is never a group member"
        );
    }

    /// Tool with a single `runtime` prerequisite — probes the `"runtime" => false`
    /// arm directly, independent of the unrecognised-kind fallback.
    struct RuntimePrereqTool;

    #[async_trait]
    impl Tool for RuntimePrereqTool {
        fn name(&self) -> &str {
            "runtime_prereq"
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "tool with a runtime prerequisite"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "runtime_prereq",
                "tool with a runtime prerequisite",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
        fn prerequisites(&self) -> Vec<Prerequisite> {
            vec![Prerequisite::runtime(
                "runtime_probe",
                "probe for the runtime satisfaction arm",
            )]
        }
    }

    /// `prerequisite_satisfied` answers `false` for `"runtime"` — via the default
    /// `is_available()`, which ANDs every ungrouped `required: true` prerequisite.
    #[test]
    fn prerequisite_satisfied_runtime_kind_is_always_false() {
        let available = RuntimePrereqTool.is_available();
        assert!(
            !available,
            "a `runtime` prerequisite must gate is_available() closed (unconditional false)"
        );
    }

    /// `prerequisite_satisfied` still answers `true` for a kind that is neither
    /// `"env_var"`, `"config_field"`, nor `"runtime"` — the `"runtime"` arm must not
    /// have widened the fallback (D-25 stays intact).
    #[test]
    fn prerequisite_satisfied_unrecognized_kind_still_true_after_runtime_arm() {
        let available = UnknownKindPrereqTool.is_available();
        assert!(
            available,
            "adding the \"runtime\" arm must not change unknown-kind non-blocking behavior (D-25)"
        );
    }

    /// D-18: the gated-closed path (Task 2's `prerequisite_satisfied` change) must
    /// log loudly — a tool vanishing from the model's schema is otherwise
    /// invisible. Asserts the captured warning names both the tool
    /// (`ConfigFieldPrereqTool::name()` == "config_field_prereq") and the
    /// prerequisite's dotted path (`"search.brave_api_key"`), and that it contains
    /// no configured value (there is none to leak here — the tool has no snapshot
    /// at all).
    #[test]
    #[tracing_test::traced_test]
    fn config_field_gating_emits_a_warning_naming_the_tool_and_the_path() {
        let available = ConfigFieldPrereqTool.is_available();
        assert!(!available);
        assert!(
            logs_contain("config_field_prereq"),
            "warning must name the tool"
        );
        assert!(
            logs_contain("search.brave_api_key"),
            "warning must name the prerequisite's dotted path"
        );
    }

    /// D-18: the positive/negative pair proving the arm gates on real snapshot
    /// state, not always-closed. Builds a real `ToolCredentials` via
    /// `resolve()` (not a hand-rolled fake) so the test exercises the actual
    /// `config_field_present()` semantics the arm depends on.
    #[tokio::test]
    async fn config_field_prereq_is_satisfied_when_the_snapshot_has_the_path() {
        const PROBE_PATH: &str = "tools.credentials.PLAN10_D18_PROBE_KEY";

        let mut config_with_value = ironhermes_core::Config::default();
        config_with_value
            .tools
            .credentials
            .insert("PLAN10_D18_PROBE_KEY".to_string(), "present".to_string());
        let creds_present = crate::credentials::ToolCredentials::resolve(&config_with_value, None)
            .await
            .expect("resolve must succeed");

        let creds_missing =
            crate::credentials::ToolCredentials::resolve(&ironhermes_core::Config::default(), None)
                .await
                .expect("resolve must succeed");

        struct SnapshotFixture {
            creds: crate::credentials::ToolCredentials,
        }

        #[async_trait]
        impl Tool for SnapshotFixture {
            fn name(&self) -> &str {
                "snapshot_fixture"
            }
            fn toolset(&self) -> &str {
                "test"
            }
            fn description(&self) -> &str {
                "fixture carrying a real ToolCredentials snapshot"
            }
            fn schema(&self) -> ToolSchema {
                ToolSchema::new(
                    "snapshot_fixture",
                    "fixture carrying a real ToolCredentials snapshot",
                    serde_json::json!({ "type": "object", "properties": {} }),
                )
            }
            async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
                Ok("ok".to_string())
            }
            fn credentials(&self) -> Option<&crate::credentials::ToolCredentials> {
                Some(&self.creds)
            }
            fn prerequisites(&self) -> Vec<Prerequisite> {
                vec![Prerequisite {
                    kind: "config_field".to_string(),
                    name: PROBE_PATH.to_string(),
                    description: "D-18 positive/negative probe".to_string(),
                    required: true,
                    group: None,
                }]
            }
        }

        let present_tool = SnapshotFixture {
            creds: creds_present,
        };
        let missing_tool = SnapshotFixture {
            creds: creds_missing,
        };

        assert!(
            present_tool.is_available(),
            "a config_field prereq must gate OPEN when the snapshot has the path"
        );
        assert!(
            !missing_tool.is_available(),
            "a config_field prereq must gate CLOSED when the snapshot lacks the path"
        );
    }

    /// D-18: the executable form of "an unconfigured provider is never offered to
    /// the model" — `get_definitions()` is the layer that decides the LLM-visible
    /// schema (`.filter(|t| t.is_available())`), so this asserts on the schema
    /// list itself, not on `is_available()` directly.
    #[test]
    fn a_gated_tool_is_absent_from_get_definitions() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ConfigFieldPrereqTool));

        let schemas = registry.get_definitions(None);

        assert!(
            !schemas.iter().any(|s| s.function.name == "config_field_prereq"),
            "a config_field-gated tool with no snapshot must be absent from get_definitions()"
        );
    }

    /// D-18: re-exercises the SHAPE of Plan 06's group scenarios (any-one-present /
    /// none-present / all-present) against the amended `is_available()`, proving
    /// the config_field arm change is scoped to its own arm and does not alter
    /// group (any-of) semantics. Deliberately does NOT reuse Plan 06's
    /// `MultiProviderFake`/`GROUP_KEYS` (private to `prerequisite_group_tests`,
    /// left untouched by this task) — this fixture and its env-var names are
    /// this test's own, added alongside (not inside) that module per the plan.
    struct GroupRegressionProbeFake;

    #[async_trait]
    impl Tool for GroupRegressionProbeFake {
        fn name(&self) -> &str {
            "group_regression_probe_fake"
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "D-18 Task 2 regression probe for D-09 group semantics"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "group_regression_probe_fake",
                "D-18 Task 2 regression probe for D-09 group semantics",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
        fn prerequisites(&self) -> Vec<Prerequisite> {
            vec![
                Prerequisite::grouped_env_var(
                    "PLAN10_REGRESSION_PROBE_A_KEY",
                    "probe provider A",
                    "plan10_regression_probe_group",
                ),
                Prerequisite::grouped_env_var(
                    "PLAN10_REGRESSION_PROBE_B_KEY",
                    "probe provider B",
                    "plan10_regression_probe_group",
                ),
            ]
        }
    }

    const REGRESSION_PROBE_KEYS: [&str; 2] = [
        "PLAN10_REGRESSION_PROBE_A_KEY",
        "PLAN10_REGRESSION_PROBE_B_KEY",
    ];

    #[test]
    fn grouped_availability_is_unchanged_by_this_task() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            for k in REGRESSION_PROBE_KEYS {
                std::env::remove_var(k);
            }
        }
        let none_present = GroupRegressionProbeFake.is_available();

        unsafe { std::env::set_var("PLAN10_REGRESSION_PROBE_A_KEY", "1") };
        let one_present = GroupRegressionProbeFake.is_available();

        unsafe { std::env::set_var("PLAN10_REGRESSION_PROBE_B_KEY", "1") };
        let all_present = GroupRegressionProbeFake.is_available();

        unsafe {
            for k in REGRESSION_PROBE_KEYS {
                std::env::remove_var(k);
            }
        }

        assert!(
            !none_present,
            "no group member present -> unavailable (D-09 unchanged)"
        );
        assert!(
            one_present,
            "any one group member present -> available (D-09 unchanged)"
        );
        assert!(
            all_present,
            "all group members present -> available (D-09 unchanged)"
        );
    }

    /// D-18 shape guard: every `config_field` prerequisite returned by a tool in
    /// the DEFAULT registry (`register_defaults()`) must name a dotted path (no
    /// whitespace, at least one `.`) — `browser_vision` is the single documented
    /// exemption (its `config_field` entry is prose, `"auxiliary.vision OR
    /// multimodal-capable main provider"`, and it overrides `is_available()`, so
    /// it never reaches this arm). `register_defaults()` does not register
    /// `browser_vision` (it needs a browser session/resolver, wired via
    /// `register_browser_tools` instead), so re-running
    /// `grep -rn 'kind: "config_field"' crates/` at execution time (recorded in
    /// the SUMMARY) confirms this loop currently finds zero `config_field`
    /// prerequisites in the default registry — this test guards the SHAPE for
    /// Plans 07/08's future credentials, not a live count today.
    #[test]
    fn every_config_field_prereq_in_the_default_registry_names_a_dotted_path() {
        const CONFIG_FIELD_SHAPE_EXEMPTIONS: &[&str] = &["browser_vision"];

        let mut registry = ToolRegistry::new();
        registry.register_defaults();

        for tool in registry.tools.values() {
            if CONFIG_FIELD_SHAPE_EXEMPTIONS.contains(&tool.name()) {
                continue;
            }
            for prereq in tool.prerequisites() {
                if prereq.kind != "config_field" {
                    continue;
                }
                assert!(
                    prereq.name.contains('.'),
                    "config_field prereq '{}' on tool '{}' must be a dotted path",
                    prereq.name,
                    tool.name()
                );
                assert!(
                    !prereq.name.chars().any(|c| c.is_whitespace()),
                    "config_field prereq '{}' on tool '{}' must not contain whitespace",
                    prereq.name,
                    tool.name()
                );
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Phase 41.3 Plan 11 (D-19): ToolRegistry's credential-snapshot handle —
    // installation, defaulting, and propagation into scope_to()/delegate
    // sub-registries.
    // ---------------------------------------------------------------------------

    /// A fresh registry's default snapshot must answer `has_credential` from
    /// LIVE env — proving no existing call site's behavior changes just
    /// because `ToolRegistry` now carries a credentials field.
    #[test]
    fn a_fresh_registry_has_an_env_only_snapshot() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::set_var("PLAN11_FRESH_REGISTRY_PROBE_KEY", "1") };

        let registry = ToolRegistry::new();
        let available = registry
            .credentials()
            .has_credential("PLAN11_FRESH_REGISTRY_PROBE_KEY");

        unsafe { std::env::remove_var("PLAN11_FRESH_REGISTRY_PROBE_KEY") };

        assert!(
            available,
            "a fresh registry's default snapshot must answer has_credential from live env"
        );
    }

    /// Install a snapshot carrying a config-tier credential, THEN
    /// `register_defaults()` — the registered `web_search` must report that
    /// credential through its own `Tool::credentials()` accessor, proving the
    /// snapshot reaches tools constructed after `with_credentials`.
    #[tokio::test]
    async fn with_credentials_reaches_tools_registered_afterwards() {
        let mut config = ironhermes_core::Config::default();
        config.tools.credentials.insert(
            "FIRECRAWL_API_KEY".to_string(),
            "plan11-config-tier-value".to_string(),
        );
        let creds = crate::credentials::ToolCredentials::resolve(&config, None)
            .await
            .expect("resolve must succeed");

        let mut registry = ToolRegistry::new();
        registry.with_credentials(Arc::new(creds));
        registry.register_defaults();

        let web_search = registry
            .get("web_search")
            .expect("register_defaults must register web_search");
        let snapshot = web_search
            .credentials()
            .expect("a registered web_search must carry a credentials() snapshot");
        assert!(
            snapshot.has_credential("FIRECRAWL_API_KEY"),
            "web_search registered after with_credentials must see the installed snapshot"
        );
    }

    /// A scoped clone's `credentials()` must answer identically to its
    /// parent's — the sub-registry silent-degradation guard `scope_to()`'s
    /// own doc comment describes for MCP tools, applied to credentials.
    #[test]
    fn scope_to_carries_the_snapshot_forward() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::set_var("PLAN11_SCOPE_TO_PROBE_KEY", "1") };

        let registry = ToolRegistry::new();
        let scoped = registry.scope_to(&["web".to_string()]);

        let parent_answer = registry
            .credentials()
            .has_credential("PLAN11_SCOPE_TO_PROBE_KEY");
        let scoped_answer = scoped
            .credentials()
            .has_credential("PLAN11_SCOPE_TO_PROBE_KEY");

        unsafe { std::env::remove_var("PLAN11_SCOPE_TO_PROBE_KEY") };

        assert!(parent_answer, "sanity: the probe key must have been visible");
        assert_eq!(
            parent_answer, scoped_answer,
            "a scoped clone's credentials() must answer identically to its parent's"
        );
    }

    /// A delegate sub-registry built for a subagent toolset list containing
    /// `web_search` must yield a tool whose snapshot answers identically to
    /// the parent's — the D-19 degradation `build_child_registry`'s new
    /// `credentials` parameter exists to prevent.
    #[tokio::test]
    async fn a_delegate_sub_registry_inherits_the_parent_snapshot() {
        let mut config = ironhermes_core::Config::default();
        config.tools.credentials.insert(
            "FIRECRAWL_API_KEY".to_string(),
            "plan11-delegate-inherit-value".to_string(),
        );
        let creds = Arc::new(
            crate::credentials::ToolCredentials::resolve(&config, None)
                .await
                .expect("resolve must succeed"),
        );

        let child_registry = crate::delegate_task::build_child_registry(
            &["web_search".to_string()],
            None,
            std::path::Path::new("/tmp"),
            crate::delegate_task::ChildRole::Leaf,
            0,
            &ironhermes_core::SubagentConfig::default(),
            None,
            None,
            creds.clone(),
        )
        .expect("build_child_registry must succeed");

        let web_search = child_registry
            .get("web_search")
            .expect("the sub-registry must register web_search");
        let snapshot = web_search
            .credentials()
            .expect("the sub-registry's web_search must carry a credentials() snapshot");
        assert!(
            snapshot.has_credential("FIRECRAWL_API_KEY"),
            "a delegate sub-registry's web_search must answer identically to the parent's snapshot"
        );
    }

    // ---------------------------------------------------------------------------
    // D-09 (Phase 41.3): group-aware default is_available() tests.
    //
    // All tests below mutate process env and take the shared `env_lock()`, matching
    // the Phase 25 Plan 01 convention above. Run with `--test-threads=1` (crate-wide
    // convention for env-mutating tests).
    // ---------------------------------------------------------------------------
    mod prerequisite_group_tests {
        use super::*;

        /// Generic fake multi-provider tool exercising the any-of group mechanism
        /// without referencing any real provider (D-09: "reusable by any future
        /// multi-provider tool, not special-cased to web search").
        struct MultiProviderFake;

        #[async_trait]
        impl Tool for MultiProviderFake {
            fn name(&self) -> &str {
                "multi_provider_fake"
            }
            fn toolset(&self) -> &str {
                "test"
            }
            fn description(&self) -> &str {
                "fake tool with a three-member any-of group"
            }
            fn schema(&self) -> ToolSchema {
                ToolSchema::new(
                    "multi_provider_fake",
                    "fake tool with a three-member any-of group",
                    serde_json::json!({ "type": "object", "properties": {} }),
                )
            }
            async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
                Ok("ok".to_string())
            }
            fn prerequisites(&self) -> Vec<Prerequisite> {
                vec![
                    Prerequisite::grouped_env_var("FAKE_A_KEY", "fake provider A", "fake_provider"),
                    Prerequisite::grouped_env_var("FAKE_B_KEY", "fake provider B", "fake_provider"),
                    Prerequisite::grouped_env_var("FAKE_C_KEY", "fake provider C", "fake_provider"),
                ]
            }
        }

        const GROUP_KEYS: [&str; 3] = ["FAKE_A_KEY", "FAKE_B_KEY", "FAKE_C_KEY"];

        /// SAFETY: callers must hold `env_lock()` for the duration of the call.
        unsafe fn clear_group_keys() {
            for k in GROUP_KEYS {
                // SAFETY: single-threaded test with env_lock held by the caller;
                // Rust 2024 edition requires unsafe.
                unsafe { std::env::remove_var(k) };
            }
        }

        #[test]
        fn any_one_group_member_present_makes_the_tool_available() {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            unsafe {
                clear_group_keys();
                std::env::set_var("FAKE_B_KEY", "1");
            }
            let available = MultiProviderFake.is_available();
            unsafe { clear_group_keys() };
            assert!(
                available,
                "one grouped member present must make the tool available"
            );
        }

        #[test]
        fn no_group_member_present_makes_the_tool_unavailable() {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            unsafe { clear_group_keys() };
            let available = MultiProviderFake.is_available();
            assert!(
                !available,
                "no grouped member present must make the tool unavailable"
            );
        }

        #[test]
        fn all_group_members_present_makes_the_tool_available() {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            unsafe {
                for k in GROUP_KEYS {
                    std::env::set_var(k, "1");
                }
            }
            let available = MultiProviderFake.is_available();
            unsafe { clear_group_keys() };
            assert!(
                available,
                "all grouped members present must make the tool available"
            );
        }

        /// Fake mixing one ungrouped required prereq with a three-member group, to
        /// prove the ungrouped AND still holds alongside the group's OR.
        struct MixedFake;

        #[async_trait]
        impl Tool for MixedFake {
            fn name(&self) -> &str {
                "mixed_fake"
            }
            fn toolset(&self) -> &str {
                "test"
            }
            fn description(&self) -> &str {
                "fake tool mixing an ungrouped required prereq with a group"
            }
            fn schema(&self) -> ToolSchema {
                ToolSchema::new(
                    "mixed_fake",
                    "fake tool mixing an ungrouped required prereq with a group",
                    serde_json::json!({ "type": "object", "properties": {} }),
                )
            }
            async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
                Ok("ok".to_string())
            }
            fn prerequisites(&self) -> Vec<Prerequisite> {
                vec![
                    Prerequisite::env_var("FAKE_UNGROUPED_REQUIRED_KEY", "ungrouped required key", true),
                    Prerequisite::grouped_env_var("FAKE_A_KEY", "fake provider A", "fake_provider"),
                    Prerequisite::grouped_env_var("FAKE_B_KEY", "fake provider B", "fake_provider"),
                ]
            }
        }

        #[test]
        fn ungrouped_required_prereq_still_ands_with_the_group() {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            unsafe {
                clear_group_keys();
                std::env::remove_var("FAKE_UNGROUPED_REQUIRED_KEY");
                std::env::set_var("FAKE_A_KEY", "1"); // group satisfied
            }
            let available_without_ungrouped = MixedFake.is_available();
            unsafe { std::env::set_var("FAKE_UNGROUPED_REQUIRED_KEY", "1") };
            let available_with_both = MixedFake.is_available();
            unsafe {
                clear_group_keys();
                std::env::remove_var("FAKE_UNGROUPED_REQUIRED_KEY");
            }
            assert!(
                !available_without_ungrouped,
                "group satisfied but ungrouped required key absent must be unavailable"
            );
            assert!(
                available_with_both,
                "group satisfied and ungrouped required key present must be available"
            );
        }

        /// Fake with two independent any-of groups, to prove both groups' ORs must
        /// hold simultaneously (AND-of-per-group-OR).
        struct TwoGroupsFake;

        #[async_trait]
        impl Tool for TwoGroupsFake {
            fn name(&self) -> &str {
                "two_groups_fake"
            }
            fn toolset(&self) -> &str {
                "test"
            }
            fn description(&self) -> &str {
                "fake tool with two independent any-of groups"
            }
            fn schema(&self) -> ToolSchema {
                ToolSchema::new(
                    "two_groups_fake",
                    "fake tool with two independent any-of groups",
                    serde_json::json!({ "type": "object", "properties": {} }),
                )
            }
            async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
                Ok("ok".to_string())
            }
            fn prerequisites(&self) -> Vec<Prerequisite> {
                vec![
                    Prerequisite::grouped_env_var("FAKE_G1_A_KEY", "group1 provider A", "g1"),
                    Prerequisite::grouped_env_var("FAKE_G1_B_KEY", "group1 provider B", "g1"),
                    Prerequisite::grouped_env_var("FAKE_G2_A_KEY", "group2 provider A", "g2"),
                    Prerequisite::grouped_env_var("FAKE_G2_B_KEY", "group2 provider B", "g2"),
                ]
            }
        }

        const G1_KEYS: [&str; 2] = ["FAKE_G1_A_KEY", "FAKE_G1_B_KEY"];
        const G2_KEYS: [&str; 2] = ["FAKE_G2_A_KEY", "FAKE_G2_B_KEY"];

        /// SAFETY: callers must hold `env_lock()` for the duration of the call.
        unsafe fn clear_two_group_keys() {
            for k in G1_KEYS.iter().chain(G2_KEYS.iter()) {
                // SAFETY: single-threaded test with env_lock held by the caller;
                // Rust 2024 edition requires unsafe.
                unsafe { std::env::remove_var(k) };
            }
        }

        #[test]
        fn two_independent_groups_both_must_be_satisfied() {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            unsafe {
                clear_two_group_keys();
                std::env::set_var("FAKE_G1_A_KEY", "1"); // only g1 satisfied
            }
            let only_g1 = TwoGroupsFake.is_available();
            unsafe { std::env::set_var("FAKE_G2_B_KEY", "1") }; // now both groups satisfied
            let both = TwoGroupsFake.is_available();
            unsafe { clear_two_group_keys() };
            assert!(
                !only_g1,
                "only one of two groups satisfied must be unavailable"
            );
            assert!(both, "both groups satisfied must be available");
        }

        #[test]
        fn ungrouped_optional_prereq_is_still_ignored() {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            unsafe { std::env::remove_var("TEST_PREREQ_25_01_PRESENT") };
            let available = OptionalEnvPrereqTool.is_available();
            assert!(
                available,
                "an ungrouped optional prereq must not block availability, group-aware or not"
            );
        }

        /// Reproduces the pre-existing `is_available_default_walks_prerequisites_*`
        /// scenarios against the group-aware implementation and asserts identical
        /// results, proving D-09 did not change ungrouped `env_var`/optional
        /// behavior.
        ///
        /// D-18 (Phase 41.3 Plan 10) deviation: this test originally also asserted
        /// `ConfigFieldPrereqTool.is_available()` was `true` ("config_field prereq
        /// -> available (unchanged)"). That claim is exactly the D-18 reversal —
        /// leaving it in place would assert something now definitionally false,
        /// breaking the build regardless of any "leave Plan 06's tests untouched"
        /// intent. The sub-check is removed here (not flipped in place) because
        /// it is redundant with the dedicated open/closed pair added by this task:
        /// `config_field_prereq_without_a_credential_snapshot_gates_closed` and
        /// `config_field_prereq_is_satisfied_when_the_snapshot_has_the_path`. The
        /// three `env_var`/optional sub-checks below are untouched.
        #[test]
        fn existing_ungrouped_behavior_is_unchanged() {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());

            unsafe { std::env::set_var("TEST_PREREQ_25_01_PRESENT", "1") };
            let required_present = RequiredEnvPrereqTool.is_available();

            unsafe { std::env::remove_var("TEST_PREREQ_25_01_PRESENT") };
            let required_absent = RequiredEnvPrereqTool.is_available();

            let optional_absent = OptionalEnvPrereqTool.is_available();

            assert!(
                required_present,
                "required env_var present -> available (unchanged)"
            );
            assert!(
                !required_absent,
                "required env_var absent -> unavailable (unchanged)"
            );
            assert!(
                optional_absent,
                "optional env_var absent -> available (unchanged)"
            );
        }

        #[test]
        fn satisfied_group_members_counts_present_keys() {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            unsafe {
                clear_group_keys();
                std::env::set_var("FAKE_A_KEY", "1");
                std::env::set_var("FAKE_B_KEY", "1");
            }
            let prereqs = MultiProviderFake.prerequisites();
            let satisfied = satisfied_group_members(&prereqs, "fake_provider");
            let members = group_members(&prereqs, "fake_provider");
            unsafe { clear_group_keys() };
            assert_eq!(
                satisfied, 2,
                "two of three group members set -> satisfied count 2"
            );
            assert_eq!(members.len(), 3, "group has three total members");
        }
    }

    // ---------------------------------------------------------------------------
    // Phase 25.3 Plan 05 (D-T-1 / Discretion D-2 — Option B):
    // Tool::redact_args default method tests.
    // ---------------------------------------------------------------------------

    /// Phase 25.3 D-T-1: a Tool that does NOT override redact_args inherits the
    /// default impl which returns the input verbatim (raw.clone()).
    ///
    /// The cast to `Box<dyn Tool>` confirms the method is object-safe — required
    /// because Plan 9's AgentLoop callback calls `tool.redact_args(...)` through
    /// the trait object stored in `ToolRegistry`.
    #[test]
    fn tool_redact_args_default_returns_input_verbatim() {
        struct DefaultMock;
        #[async_trait]
        impl Tool for DefaultMock {
            fn name(&self) -> &str {
                "default_mock"
            }
            fn toolset(&self) -> &str {
                "test"
            }
            fn description(&self) -> &str {
                "test mock for redact_args default"
            }
            fn schema(&self) -> ToolSchema {
                ToolSchema::new(
                    "default_mock",
                    "test mock for redact_args default",
                    serde_json::json!({ "type": "object", "properties": {} }),
                )
            }
            async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
                Ok(String::new())
            }
            // redact_args intentionally NOT overridden — exercises the default.
        }

        // Object-safety check: the default method must be callable on a trait object.
        let tool: Box<dyn Tool> = Box::new(DefaultMock);
        let raw = serde_json::json!({
            "url": "https://example.com/?api_key=sk-secret",
            "n": 42,
            "nested": {"key": "value"}
        });
        let redacted = tool.redact_args(&raw);
        assert_eq!(
            redacted, raw,
            "default redact_args must return input verbatim (no mutation)"
        );
    }

    // ---------------------------------------------------------------------------
    // Phase 48.2 Plan 11 (G-48.2-6 slice a): Tool::runtime_dependency default
    // ---------------------------------------------------------------------------

    /// A tool that does NOT override `runtime_dependency()` inherits the
    /// default `None` — the zero-edit guarantee every existing `impl Tool`
    /// block relies on. Object-safety check mirrors
    /// `tool_redact_args_default_returns_input_verbatim` above: the default
    /// method must be callable through the trait object `ToolRegistry`
    /// actually stores.
    #[test]
    fn tool_runtime_dependency_default_is_none() {
        struct NoDependencyMock;
        #[async_trait]
        impl Tool for NoDependencyMock {
            fn name(&self) -> &str {
                "no_dependency_mock"
            }
            fn toolset(&self) -> &str {
                "test"
            }
            fn description(&self) -> &str {
                "test mock for runtime_dependency default"
            }
            fn schema(&self) -> ToolSchema {
                ToolSchema::new(
                    "no_dependency_mock",
                    "test mock for runtime_dependency default",
                    serde_json::json!({ "type": "object", "properties": {} }),
                )
            }
            async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
                Ok(String::new())
            }
            // runtime_dependency intentionally NOT overridden — exercises the default.
        }

        let tool: Box<dyn Tool> = Box::new(NoDependencyMock);
        assert_eq!(tool.runtime_dependency(), None);
    }

    // ---------------------------------------------------------------------------
    // Phase 25 Plan 01 Task 2: D-01 toolset name enumeration test
    // ---------------------------------------------------------------------------

    /// Verify that every built-in tool's toolset() return value matches the D-01
    /// six-name enumeration: {web, code, memory, agent, skills, session}.
    ///
    /// For unit-struct tools (no constructor complexity), instantiate directly and
    /// assert toolset(). For CronjobTool (requires Arc<Mutex<JobStore>>), use the
    /// source-text invariant approach (include_str!) per Phase 22.3-12 pattern —
    /// verifies the literal "agent" is in the toolset() impl block.
    #[test]
    fn toolset_names_match_d01_enumeration() {
        use crate::file_tools::{PatchFileTool, ReadFileTool, SearchFilesTool, WriteFileTool};
        use crate::terminal::TerminalTool;

        // Direct instantiation for tools with trivial constructors
        assert_eq!(
            TerminalTool::new().toolset(),
            "code",
            "TerminalTool must be in 'code' toolset per D-01"
        );
        assert_eq!(
            ReadFileTool.toolset(),
            "code",
            "ReadFileTool must be in 'code' toolset per D-01"
        );
        assert_eq!(
            WriteFileTool.toolset(),
            "code",
            "WriteFileTool must be in 'code' toolset per D-01"
        );
        assert_eq!(
            PatchFileTool.toolset(),
            "code",
            "PatchFileTool must be in 'code' toolset per D-01"
        );
        assert_eq!(
            SearchFilesTool.toolset(),
            "code",
            "SearchFilesTool must be in 'code' toolset per D-01"
        );

        // Source-text invariant for CronjobTool (requires Arc<Mutex<JobStore>> constructor).
        // Verifies that the toolset() impl block returns "agent" per D-01 Open Question 1 resolution.
        let cronjob_src = include_str!("cronjob_tool.rs");
        // Find the toolset() impl block and verify "agent" literal is present
        // (and "cronjob" is NOT present as a toolset return value)
        let toolset_section: String = cronjob_src
            .lines()
            .skip_while(|l| !l.contains("fn toolset"))
            .take(5)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            toolset_section.contains("\"agent\""),
            "CronjobTool::toolset() must return \"agent\" per D-01; found:\n{toolset_section}"
        );
        assert!(
            !toolset_section.contains("\"cronjob\""),
            "CronjobTool::toolset() must NOT return \"cronjob\" (fixed by Plan 1); found:\n{toolset_section}"
        );
    }

    // ---------------------------------------------------------------------------
    // Phase 25 Plan 01 Task 3: web tool prerequisites() and is_available() tests
    // ---------------------------------------------------------------------------

    /// Test: WebSearchTool::prerequisites() returns one grouped entry per
    /// KEYED provider (Exa, Brave, Tavily), all sharing the
    /// `web_search_provider` any-of group, none independently required.
    ///
    /// REWRITTEN under Phase 41.3 Plan 07 (D-09): the pre-41.3 assertion set
    /// (`len() == 1`, a single `FIRECRAWL_API_KEY` entry, `required: true`)
    /// asserted the OLD single-provider-required shape. `web_search` is now
    /// a config-ordered multi-provider chain (Exa > Brave > Tavily > DDG)
    /// that is available with zero keys because DDG (keyless) terminates
    /// the chain — so no single key can be "required" anymore. The
    /// replacement keeps the test's original intent ("web_search's
    /// prerequisites are what the wizard will prompt for") against the new
    /// any-of shape.
    #[test]
    fn web_search_prerequisites_are_a_provider_group() {
        let tool = crate::web_search::WebSearchTool::default();
        let prereqs = tool.prerequisites();

        assert_eq!(
            prereqs.len(),
            3,
            "WebSearchTool must have exactly one grouped prerequisite per keyed provider \
             (Exa, Brave, Tavily) — DDG needs no key and contributes none"
        );

        for p in &prereqs {
            assert_eq!(
                p.group.as_deref(),
                Some(crate::web_search::WEB_SEARCH_PROVIDER_GROUP),
                "every web_search prerequisite must share the any-of provider group, got {:?}",
                p
            );
            assert!(
                !p.required,
                "a grouped prerequisite must never be independently required \
                 (grouped_env_var sets required: false by construction), got {:?}",
                p
            );
            assert_eq!(
                p.kind, "env_var",
                "web_search prerequisites are env_var-kind, got {:?}",
                p
            );
        }

        let mut names: Vec<&str> = prereqs.iter().map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec!["BRAVE_API_KEY", "EXA_API_KEY", "TAVILY_API_KEY"],
            "web_search's grouped prerequisites must name exactly the three keyed providers"
        );
    }

    /// Test: WebReadTool::prerequisites() returns one Prerequisite with
    /// kind == "env_var", name == "FIRECRAWL_API_KEY", required == false.
    #[test]
    fn web_read_prerequisites_lists_firecrawl_required_false() {
        let tool = crate::web_read::WebReadTool;
        let prereqs = tool.prerequisites();
        assert_eq!(
            prereqs.len(),
            1,
            "WebReadTool must have exactly one prerequisite"
        );
        let p = &prereqs[0];
        assert_eq!(
            p.kind, "env_var",
            "WebReadTool prereq kind must be 'env_var'"
        );
        assert_eq!(
            p.name, "FIRECRAWL_API_KEY",
            "WebReadTool prereq name must be FIRECRAWL_API_KEY"
        );
        assert!(
            !p.required,
            "WebReadTool FIRECRAWL_API_KEY prereq must be required:false (plain-text fallback)"
        );
    }

    /// Test: WebSearchTool::is_available() is `true` even with every
    /// provider key unset — DDG (keyless) terminates the default chain, so
    /// a fresh install never hard-fails on web_search (D-09).
    ///
    /// REWRITTEN under Phase 41.3 Plan 07: the pre-41.3 version of this test
    /// asserted `is_available() == false` without `FIRECRAWL_API_KEY` — the
    /// single-provider-required behavior this plan explicitly reverses.
    #[test]
    fn web_search_is_available_even_without_any_provider_key() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: single-threaded test with env_lock held; Rust 2024 edition requires unsafe.
        unsafe {
            std::env::remove_var("EXA_API_KEY");
            std::env::remove_var("BRAVE_API_KEY");
            std::env::remove_var("TAVILY_API_KEY");
        }
        let tool = crate::web_search::WebSearchTool::default();
        assert!(
            tool.is_available(),
            "WebSearchTool::is_available() must stay true with zero provider keys — DDG \
             terminates the chain (D-09)"
        );
    }

    /// Test: With FIRECRAWL_API_KEY unset, WebReadTool::is_available() == true
    /// (required:false does not block; web_read has plain-text fallback per D-09).
    #[test]
    fn web_read_is_available_stays_true_without_firecrawl() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: single-threaded test with env_lock held; Rust 2024 edition requires unsafe.
        unsafe { std::env::remove_var("FIRECRAWL_API_KEY") };
        let tool = crate::web_read::WebReadTool;
        let available = tool.is_available();
        assert!(
            available,
            "WebReadTool::is_available() must be true when FIRECRAWL_API_KEY is unset (optional prereq)"
        );
    }

    // ---------------------------------------------------------------------------
    // register_dynamic tests (D-10)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_register_dynamic_inserts_tool() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic(Box::new(MockTool {
            tool_name: "dyn_tool",
        }));
        assert!(
            registry.get("dyn_tool").is_some(),
            "dynamically registered tool must be retrievable by name"
        );
    }

    #[test]
    fn test_register_dynamic_overwrites_existing() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic(Box::new(MockTool {
            tool_name: "my_tool",
        }));
        registry.register_dynamic(Box::new(MockTool {
            tool_name: "my_tool",
        }));
        // Should still be exactly one tool named "my_tool"
        let names = registry.list_tools();
        let count = names.iter().filter(|&&n| n == "my_tool").count();
        assert_eq!(count, 1, "register_dynamic must overwrite, not duplicate");
    }

    // ---------------------------------------------------------------------------
    // unregister_by_prefix tests (D-10)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_unregister_by_prefix_removes_matching_tools() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic(Box::new(MockTool {
            tool_name: "server__tool_a",
        }));
        registry.register_dynamic(Box::new(MockTool {
            tool_name: "server__tool_b",
        }));
        registry.register_dynamic(Box::new(MockTool {
            tool_name: "other__tool_c",
        }));

        let removed = registry.unregister_by_prefix("server");
        assert_eq!(removed, 2, "must remove both 'server__' prefixed tools");
        assert!(
            registry.get("server__tool_a").is_none(),
            "server__tool_a must be removed"
        );
        assert!(
            registry.get("server__tool_b").is_none(),
            "server__tool_b must be removed"
        );
    }

    #[test]
    fn test_unregister_by_prefix_does_not_remove_other_tools() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic(Box::new(MockTool {
            tool_name: "server__tool_a",
        }));
        registry.register_dynamic(Box::new(MockTool {
            tool_name: "other__tool_c",
        }));

        registry.unregister_by_prefix("server");
        assert!(
            registry.get("other__tool_c").is_some(),
            "other__tool_c must NOT be removed"
        );
    }

    #[test]
    fn test_unregister_by_prefix_returns_count() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic(Box::new(MockTool {
            tool_name: "srv__a",
        }));
        registry.register_dynamic(Box::new(MockTool {
            tool_name: "srv__b",
        }));
        registry.register_dynamic(Box::new(MockTool {
            tool_name: "srv__c",
        }));

        let count = registry.unregister_by_prefix("srv");
        assert_eq!(
            count, 3,
            "unregister_by_prefix must return count of removed tools"
        );
    }

    #[test]
    fn test_unregister_by_prefix_empty_registry_returns_zero() {
        let mut registry = ToolRegistry::new();
        let count = registry.unregister_by_prefix("server");
        assert_eq!(
            count, 0,
            "unregister_by_prefix on empty registry must return 0"
        );
    }

    #[test]
    fn test_unregister_by_prefix_no_match_returns_zero() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic(Box::new(MockTool {
            tool_name: "other__tool",
        }));
        let count = registry.unregister_by_prefix("x");
        assert_eq!(
            count, 0,
            "unregister_by_prefix with no matching prefix must return 0"
        );
    }

    // ---------------------------------------------------------------------------
    // Phase 36.3.7.13 D-B2 — retain_by_name tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_retain_by_name_removes_non_allowed() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic(Box::new(MockTool { tool_name: "a" }));
        registry.register_dynamic(Box::new(MockTool { tool_name: "b" }));
        registry.register_dynamic(Box::new(MockTool { tool_name: "c" }));

        let removed = registry.retain_by_name(&["a", "b"]);

        assert_eq!(
            removed, 1,
            "retain_by_name must return the count of removed tools"
        );
        let names = registry.list_tools();
        assert!(
            names.contains(&"a"),
            "\"a\" must remain after retain_by_name"
        );
        assert!(
            names.contains(&"b"),
            "\"b\" must remain after retain_by_name"
        );
        assert!(
            !names.contains(&"c"),
            "\"c\" must be removed by retain_by_name (not in allowlist)"
        );
    }

    #[test]
    fn test_retain_by_name_returns_removed_count() {
        let mut registry = ToolRegistry::new();
        for name in &["x", "y", "z", "w"] {
            registry.register_dynamic(Box::new(MockTool { tool_name: name }));
        }

        // Allow only "x" → removes 3.
        let removed = registry.retain_by_name(&["x"]);
        assert_eq!(
            removed, 3,
            "retain_by_name must return number of removed tools (4 total − 1 allowed = 3 removed)"
        );
        let names = registry.list_tools();
        assert_eq!(names.len(), 1, "only one tool must remain");
        assert_eq!(names[0], "x");
    }

    // ---------------------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_dispatch_with_no_guardrails_passes() {
        let registry = make_registry_with_tool("test_tool");
        let result = registry
            .dispatch("test_tool", serde_json::Value::Null)
            .await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        assert_eq!(result.unwrap(), "mock result");
    }

    #[tokio::test]
    async fn test_dispatch_blocked_by_guardrail() {
        let mut registry = make_registry_with_tool("test_tool");
        registry.add_guardrail(Box::new(BlocklistGuardrail::new(vec![
            "test_tool".to_string(),
        ])));

        let result = registry
            .dispatch("test_tool", serde_json::Value::Null)
            .await;
        assert!(result.is_err(), "expected Err (blocked), got Ok");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.to_lowercase().contains("blocked")
                || err_msg.contains("blocklist")
                || err_msg.contains("security policy"),
            "error should mention block: {err_msg}"
        );
    }

    #[tokio::test]
    async fn test_dispatch_allowed_by_guardrail() {
        let mut registry = make_registry_with_tool("test_tool");
        registry.add_guardrail(Box::new(BlocklistGuardrail::new(vec![
            "other_tool".to_string(),
        ])));

        let result = registry
            .dispatch("test_tool", serde_json::Value::Null)
            .await;
        assert!(result.is_ok(), "expected Ok (allowed), got {result:?}");
        assert_eq!(result.unwrap(), "mock result");
    }

    #[tokio::test]
    async fn test_dispatch_warn_guardrail_proceeds() {
        let mut registry = make_registry_with_tool("test_tool");
        registry.add_guardrail(Box::new(WarnGuardrail));

        let result = registry
            .dispatch("test_tool", serde_json::Value::Null)
            .await;
        assert!(result.is_ok(), "warn guardrail must not block: {result:?}");
        assert_eq!(result.unwrap(), "mock result");
    }

    // ---------------------------------------------------------------------------
    // Phase 25 Plan 02 Task 1: InterceptHandler + register_intercepted + dispatch_intercepts tests
    // ---------------------------------------------------------------------------

    fn test_handler(response: &'static str) -> InterceptHandler {
        std::sync::Arc::new(move |_args| Box::pin(async move { Ok(response.to_string()) }))
    }

    fn test_intercept_schema(name: &str) -> ToolSchema {
        ToolSchema::new(
            name,
            "test intercept tool",
            serde_json::json!({ "type": "object", "properties": {} }),
        )
    }

    /// Test: register_intercepted inserts schema; get_definitions(None) includes it exactly once.
    #[test]
    fn register_intercepted_inserts_schema_and_handler() {
        let mut registry = ToolRegistry::new();
        registry.register_intercepted(
            "test_intercept",
            test_intercept_schema("test_intercept"),
            test_handler("hello"),
        );
        let schemas = registry.get_definitions(None);
        let count = schemas
            .iter()
            .filter(|s| s.function.name == "test_intercept")
            .count();
        assert_eq!(
            count, 1,
            "intercepted tool must appear exactly once in get_definitions(None)"
        );
    }

    /// Test: register_intercepted panics when name already registered as a regular tool (D-15).
    #[test]
    #[should_panic(expected = "already registered as a regular tool")]
    fn register_intercepted_panics_on_duplicate_with_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MockTool {
            tool_name: "dup_name",
        }));
        registry.register_intercepted(
            "dup_name",
            test_intercept_schema("dup_name"),
            test_handler("x"),
        );
    }

    /// Test: register() panics when name already registered as an intercepted tool (D-15 reciprocal).
    #[test]
    #[should_panic(expected = "already registered as an intercepted tool")]
    fn register_tools_panics_on_duplicate_with_intercepts() {
        let mut registry = ToolRegistry::new();
        registry.register_intercepted(
            "dup_name",
            test_intercept_schema("dup_name"),
            test_handler("x"),
        );
        registry.register(Box::new(MockTool {
            tool_name: "dup_name",
        }));
    }

    /// Test: dispatch_intercepts returns Some(Ok("hello")) for a known intercepted tool.
    /// `register_intercepted` is the session-agnostic legacy API — it stores under
    /// `LEGACY_GLOBAL_INTERCEPT_SESSION`, so dispatch succeeds for ANY session_id
    /// (Phase 36.3.12 CR-01 fallback semantics).
    #[tokio::test]
    async fn dispatch_intercepts_returns_some_for_known() {
        let mut registry = ToolRegistry::new();
        registry.register_intercepted(
            "known",
            test_intercept_schema("known"),
            test_handler("hello"),
        );
        let result = registry
            .dispatch_intercepts("known", "any-session", serde_json::json!({}))
            .await;
        assert!(
            result.is_some(),
            "dispatch_intercepts must return Some for a known intercepted name"
        );
        let inner = result.unwrap();
        assert!(inner.is_ok(), "handler must return Ok");
        assert_eq!(inner.unwrap(), "hello");
    }

    /// Test: dispatch_intercepts returns None for an unknown name (caller falls through to dispatch()).
    #[tokio::test]
    async fn dispatch_intercepts_returns_none_for_unknown() {
        let registry = ToolRegistry::new();
        let result = registry
            .dispatch_intercepts("unknown", "any-session", serde_json::json!({}))
            .await;
        assert!(
            result.is_none(),
            "dispatch_intercepts must return None for an unregistered name"
        );
    }

    // ---------------------------------------------------------------------------
    // Phase 36.3.12 Plan 09 (CR-01): session-scoped register_intercepted_or_replace
    // + dispatch_intercepts + unregister_intercepts_for_session tests.
    // See 36.3.12-REVIEW.md CR-01 for the exact cross-session dispatch scenario these
    // tests guard against.
    // ---------------------------------------------------------------------------

    /// Behavior 1: two sessions' intercepts for the SAME tool name coexist — neither
    /// overwrites the other, and each dispatch resolves to its own registering session's
    /// handler output.
    #[tokio::test]
    async fn register_intercepted_or_replace_two_sessions_coexist() {
        let mut registry = ToolRegistry::new();
        registry.register_intercepted_or_replace(
            "terminal",
            "sess-a",
            test_intercept_schema("terminal"),
            test_handler("sess-a-output"),
        );
        registry.register_intercepted_or_replace(
            "terminal",
            "sess-b",
            test_intercept_schema("terminal"),
            test_handler("sess-b-output"),
        );

        let a = registry
            .dispatch_intercepts("terminal", "sess-a", serde_json::json!({}))
            .await
            .expect("sess-a must have a registered handler")
            .expect("sess-a's handler must succeed");
        assert_eq!(
            a, "sess-a-output",
            "dispatching for sess-a must invoke sess-a's own handler, not sess-b's (CR-01)"
        );

        let b = registry
            .dispatch_intercepts("terminal", "sess-b", serde_json::json!({}))
            .await
            .expect("sess-b must have a registered handler")
            .expect("sess-b's handler must succeed");
        assert_eq!(
            b, "sess-b-output",
            "dispatching for sess-b must invoke sess-b's own handler, not sess-a's (CR-01)"
        );
    }

    /// Behavior 2: re-registering the SAME session replaces its handler in place and
    /// does not disturb a different session's handler for the same tool name.
    #[tokio::test]
    async fn register_intercepted_or_replace_same_session_replaces_in_place() {
        let mut registry = ToolRegistry::new();
        registry.register_intercepted_or_replace(
            "terminal",
            "sess-a",
            test_intercept_schema("terminal"),
            test_handler("first"),
        );
        registry.register_intercepted_or_replace(
            "terminal",
            "sess-b",
            test_intercept_schema("terminal"),
            test_handler("sess-b-output"),
        );
        // Re-register sess-a with a different handler.
        registry.register_intercepted_or_replace(
            "terminal",
            "sess-a",
            test_intercept_schema("terminal"),
            test_handler("second"),
        );

        let a = registry
            .dispatch_intercepts("terminal", "sess-a", serde_json::json!({}))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            a, "second",
            "re-registering sess-a must replace its handler in place"
        );

        let b = registry
            .dispatch_intercepts("terminal", "sess-b", serde_json::json!({}))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            b, "sess-b-output",
            "sess-b's handler must be undisturbed by sess-a's re-registration"
        );
    }

    /// Behavior 3 (D-10 fail-closed): dispatching an intercepted name with a session
    /// that has NO registered handler returns `Some(Err(..))` — never `None`, which
    /// would let the caller fall through to the raw ungated tool.
    #[tokio::test]
    async fn dispatch_intercepts_session_miss_on_intercepted_name_returns_err() {
        let mut registry = ToolRegistry::new();
        registry.register_intercepted_or_replace(
            "terminal",
            "sess-a",
            test_intercept_schema("terminal"),
            test_handler("sess-a-output"),
        );

        let result = registry
            .dispatch_intercepts("terminal", "sess-ghost", serde_json::json!({}))
            .await;
        assert!(
            result.is_some(),
            "a session miss on an intercepted name must be Some(Err), never None (D-10 fail-closed)"
        );
        let inner = result.unwrap();
        assert!(
            inner.is_err(),
            "a session miss on an intercepted name must not silently succeed or fall through"
        );
        let msg = inner.unwrap_err().to_string();
        assert!(msg.contains("terminal"), "error must name the tool: {msg}");
        assert!(
            msg.contains("sess-ghost"),
            "error must name the missing session: {msg}"
        );
    }

    /// Behavior 4: a name that was NEVER intercepted still returns `None` — the caller
    /// must fall through to the normal `dispatch()` path for ordinary tools.
    #[tokio::test]
    async fn dispatch_intercepts_never_intercepted_name_returns_none() {
        let mut registry = ToolRegistry::new();
        registry.register_intercepted_or_replace(
            "terminal",
            "sess-a",
            test_intercept_schema("terminal"),
            test_handler("sess-a-output"),
        );

        let result = registry
            .dispatch_intercepts("some_other_tool", "sess-a", serde_json::json!({}))
            .await;
        assert!(
            result.is_none(),
            "a name that was never intercepted must return None so the caller falls through to dispatch()"
        );
    }

    /// Behavior 5: after registering the SAME tool name for three different sessions,
    /// get_definitions() still advertises exactly ONE schema entry for that name.
    #[tokio::test]
    async fn get_definitions_one_schema_per_name_regardless_of_session_count() {
        let mut registry = ToolRegistry::new();
        for sess in ["sess-a", "sess-b", "sess-c"] {
            registry.register_intercepted_or_replace(
                "terminal",
                sess,
                test_intercept_schema("terminal"),
                test_handler(sess),
            );
        }

        let schemas = registry.get_definitions(None);
        let count = schemas
            .iter()
            .filter(|s| s.function.name == "terminal")
            .count();
        assert_eq!(
            count, 1,
            "get_definitions() must advertise exactly one schema per intercepted name \
             regardless of how many sessions have live handlers registered"
        );
    }

    /// Behavior 6: unregister_intercepts_for_session removes ONLY that session's
    /// handlers and leaves other sessions' handlers dispatchable.
    #[tokio::test]
    async fn unregister_intercepts_for_session_removes_only_that_session() {
        let mut registry = ToolRegistry::new();
        registry.register_intercepted_or_replace(
            "terminal",
            "sess-a",
            test_intercept_schema("terminal"),
            test_handler("sess-a-output"),
        );
        registry.register_intercepted_or_replace(
            "terminal",
            "sess-b",
            test_intercept_schema("terminal"),
            test_handler("sess-b-output"),
        );

        registry.unregister_intercepts_for_session("sess-a");

        let a = registry
            .dispatch_intercepts("terminal", "sess-a", serde_json::json!({}))
            .await
            .expect("name is still intercepted (sess-b remains) so this must be Some");
        assert!(
            a.is_err(),
            "sess-a's handler must be gone after unregister_intercepts_for_session(\"sess-a\")"
        );

        let b = registry
            .dispatch_intercepts("terminal", "sess-b", serde_json::json!({}))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            b, "sess-b-output",
            "sess-b's handler must survive unregistering sess-a"
        );
    }

    // ---------------------------------------------------------------------------
    // Phase 25 Plan 02 Task 2: get_definitions intercept union + list_unavailable + list_toolsets
    // ---------------------------------------------------------------------------

    /// Test: get_definitions(None) includes schemas from both regular tools and intercepted tools.
    #[test]
    fn get_definitions_includes_intercept_schemas() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MockTool {
            tool_name: "regular",
        }));
        registry.register_intercepted(
            "intercept_a",
            test_intercept_schema("intercept_a"),
            test_handler("a"),
        );
        registry.register_intercepted(
            "intercept_b",
            test_intercept_schema("intercept_b"),
            test_handler("b"),
        );
        let schemas = registry.get_definitions(None);
        let names: std::collections::HashSet<String> =
            schemas.iter().map(|s| s.function.name.clone()).collect();
        assert_eq!(names.len(), 3, "must have 3 schemas: {names:?}");
        assert!(names.contains("regular"), "missing 'regular'");
        assert!(names.contains("intercept_a"), "missing 'intercept_a'");
        assert!(names.contains("intercept_b"), "missing 'intercept_b'");
    }

    /// Test: enabled_tools filter applies to both regular tools and intercepted tools.
    #[test]
    fn get_definitions_with_enabled_tools_filter_includes_intercepts() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MockTool {
            tool_name: "regular",
        }));
        registry.register_intercepted(
            "intercept_a",
            test_intercept_schema("intercept_a"),
            test_handler("a"),
        );
        registry.register_intercepted(
            "intercept_b",
            test_intercept_schema("intercept_b"),
            test_handler("b"),
        );
        let enabled = vec!["regular".to_string(), "intercept_a".to_string()];
        let schemas = registry.get_definitions(Some(&enabled));
        let names: std::collections::HashSet<String> =
            schemas.iter().map(|s| s.function.name.clone()).collect();
        assert_eq!(
            names.len(),
            2,
            "must have 2 schemas after filter: {names:?}"
        );
        assert!(names.contains("regular"), "missing 'regular'");
        assert!(names.contains("intercept_a"), "missing 'intercept_a'");
        assert!(
            !names.contains("intercept_b"),
            "'intercept_b' must be filtered out"
        );
    }

    /// Tool whose is_available() returns false — used to test filtering.
    struct UnavailableTool;

    #[async_trait]
    impl Tool for UnavailableTool {
        fn name(&self) -> &str {
            "unavailable"
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "always unavailable"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "unavailable",
                "always unavailable",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        fn is_available(&self) -> bool {
            false
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("never called".to_string())
        }
    }

    /// Test: unavailable regular tools are filtered out; intercepted tools (no is_available) always appear.
    #[test]
    fn get_definitions_filters_unavailable_regular_tools_only() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(UnavailableTool));
        registry.register_intercepted(
            "always_on",
            test_intercept_schema("always_on"),
            test_handler("ok"),
        );
        let schemas = registry.get_definitions(None);
        let names: Vec<String> = schemas.iter().map(|s| s.function.name.clone()).collect();
        assert!(
            !names.contains(&"unavailable".to_string()),
            "unavailable regular tool must be filtered out; got: {names:?}"
        );
        assert!(
            names.contains(&"always_on".to_string()),
            "intercepted tool must always appear in get_definitions; got: {names:?}"
        );
        assert_eq!(
            names.len(),
            1,
            "only intercepted tool should appear; got: {names:?}"
        );
    }

    /// Tool B with a required env_var prerequisite for list_unavailable testing.
    struct MissingKeyTool;

    #[async_trait]
    impl Tool for MissingKeyTool {
        fn name(&self) -> &str {
            "test_b"
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "tool requiring MISSING_KEY_25_02"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "test_b",
                "tool requiring MISSING_KEY_25_02",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
        fn prerequisites(&self) -> Vec<Prerequisite> {
            vec![Prerequisite {
                kind: "env_var".to_string(),
                name: "MISSING_KEY_25_02".to_string(),
                description: "Test key for Phase 25 Plan 02 list_unavailable test.".to_string(),
                required: true,
                group: None,
            }]
        }
    }

    /// Tool A with no prerequisites for list_unavailable testing.
    struct AlwaysAvailTool;

    #[async_trait]
    impl Tool for AlwaysAvailTool {
        fn name(&self) -> &str {
            "test_a"
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "always available tool"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "test_a",
                "always available tool",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
    }

    /// Test: list_unavailable() returns tools with missing required prerequisites.
    #[test]
    fn list_unavailable_returns_missing_required_prereqs() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: single-threaded test with env_lock held; Rust 2024 edition requires unsafe.
        unsafe { std::env::remove_var("MISSING_KEY_25_02") };
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(AlwaysAvailTool));
        registry.register(Box::new(MissingKeyTool));
        let unavailable = registry.list_unavailable();
        assert_eq!(
            unavailable.len(),
            1,
            "exactly one tool must be unavailable when MISSING_KEY_25_02 is unset; got: {unavailable:?}"
        );
        let (tool_name, missing) = &unavailable[0];
        assert_eq!(
            tool_name.as_str(),
            "test_b",
            "the unavailable tool must be 'test_b'; got: {tool_name}"
        );
        assert_eq!(
            missing.len(),
            1,
            "must have exactly one missing prereq; got: {missing:?}"
        );

        // With env set, returns empty Vec
        unsafe { std::env::set_var("MISSING_KEY_25_02", "1") };
        let unavailable_after = registry.list_unavailable();
        unsafe { std::env::remove_var("MISSING_KEY_25_02") };
        assert!(
            unavailable_after.is_empty(),
            "list_unavailable must return empty when all prereqs satisfied; got: {unavailable_after:?}"
        );
    }

    /// Tools with different toolsets for list_toolsets testing.
    struct WebTool1;
    struct CodeTool1;
    struct WebTool2;

    #[async_trait]
    impl Tool for WebTool1 {
        fn name(&self) -> &str {
            "web_tool_1"
        }
        fn toolset(&self) -> &str {
            "web"
        }
        fn description(&self) -> &str {
            "web tool 1"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "web_tool_1",
                "web tool 1",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
    }

    #[async_trait]
    impl Tool for CodeTool1 {
        fn name(&self) -> &str {
            "code_tool_1"
        }
        fn toolset(&self) -> &str {
            "code"
        }
        fn description(&self) -> &str {
            "code tool 1"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "code_tool_1",
                "code tool 1",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
    }

    #[async_trait]
    impl Tool for WebTool2 {
        fn name(&self) -> &str {
            "web_tool_2"
        }
        fn toolset(&self) -> &str {
            "web"
        }
        fn description(&self) -> &str {
            "web tool 2"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "web_tool_2",
                "web tool 2",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
    }

    /// Test: list_toolsets() returns unique, sorted toolset names from regular tools.
    #[test]
    fn list_toolsets_returns_unique_set() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(WebTool1));
        registry.register(Box::new(CodeTool1));
        registry.register(Box::new(WebTool2));
        let toolsets = registry.list_toolsets();
        assert_eq!(
            toolsets,
            vec!["code", "web"],
            "list_toolsets must return deduplicated, sorted toolset names; got: {toolsets:?}"
        );
    }

    // ---------------------------------------------------------------------------
    // Phase 25 Plan 02 Task 3: todo_write_schema, todo_read_schema, D-26 Test 3
    // ---------------------------------------------------------------------------

    /// Test: todo_write_schema() returns a ToolSchema with name "todo_write" and
    /// a required "items" field of type array of strings.
    #[test]
    fn todo_write_schema_minimal_shape() {
        let schema = crate::registry::todo_write_schema();
        assert_eq!(
            schema.function.name, "todo_write",
            "todo_write_schema must have name 'todo_write'"
        );
        let params = serde_json::to_value(&schema.function.parameters).unwrap();
        let props = &params["properties"];
        assert!(
            props.get("items").is_some(),
            "todo_write_schema must have 'items' in properties; got: {props}"
        );
        let items_type = props["items"]["type"].as_str().unwrap_or("");
        assert_eq!(
            items_type, "array",
            "todo_write_schema 'items' must be of type 'array'; got: {items_type}"
        );
        let item_item_type = props["items"]["items"]["type"].as_str().unwrap_or("");
        assert_eq!(
            item_item_type, "string",
            "todo_write_schema items.items.type must be 'string'; got: {item_item_type}"
        );
        let required = params["required"].as_array().unwrap();
        assert!(
            required.iter().any(|v| v.as_str() == Some("items")),
            "todo_write_schema must have 'items' in required; got: {required:?}"
        );
    }

    /// Test: todo_read_schema() returns a ToolSchema with name "todo_read"
    /// and empty/no-arg parameters.
    #[test]
    fn todo_read_schema_minimal_shape() {
        let schema = crate::registry::todo_read_schema();
        assert_eq!(
            schema.function.name, "todo_read",
            "todo_read_schema must have name 'todo_read'"
        );
        let params = serde_json::to_value(&schema.function.parameters).unwrap();
        let props = params["properties"].as_object();
        assert!(
            props.map(|p| p.is_empty()).unwrap_or(true),
            "todo_read_schema must have empty properties; got: {params}"
        );
        // No required fields (or empty required array)
        let required = params.get("required").and_then(|r| r.as_array());
        assert!(
            required.map(|r| r.is_empty()).unwrap_or(true),
            "todo_read_schema must have no required fields; got: {params}"
        );
    }

    /// D-26 Test 3 (mandatory): intercepted_tool_no_schema_duplicate.
    /// Boot registry with all 6 intercepted tool names; assert each appears
    /// exactly once in get_definitions(None).
    #[tokio::test]
    async fn intercepted_tool_no_schema_duplicate() {
        let mut registry = ToolRegistry::new();
        let names = [
            "memory",
            "session_search",
            "delegate_task",
            "todo_write",
            "todo_read",
            "cronjob",
        ];
        for name in names {
            registry.register_intercepted(
                name,
                ToolSchema::new(
                    name,
                    "stub intercepted tool for D-26 Test 3",
                    serde_json::json!({ "type": "object", "properties": {} }),
                ),
                std::sync::Arc::new(|_args| Box::pin(async move { Ok("stub".to_string()) })),
            );
        }
        let schemas = registry.get_definitions(None);
        let names_returned: Vec<String> = schemas.iter().map(|s| s.function.name.clone()).collect();
        for name in names {
            let count = names_returned.iter().filter(|n| n.as_str() == name).count();
            assert_eq!(
                count, 1,
                "intercepted tool '{}' must appear exactly once in schema list, found {}; all: {:?}",
                name, count, names_returned
            );
        }
    }

    // ---------------------------------------------------------------------------
    // Phase 25 Plan 03 Task 2: toolset_config filter + D-15 collision guard tests
    // ---------------------------------------------------------------------------

    /// MockTool with a configurable toolset name for toolset-filter tests.
    struct ToolsetMockTool {
        tool_name: &'static str,
        toolset_name: &'static str,
    }

    #[async_trait]
    impl Tool for ToolsetMockTool {
        fn name(&self) -> &str {
            self.tool_name
        }
        fn toolset(&self) -> &str {
            self.toolset_name
        }
        fn description(&self) -> &str {
            "toolset mock tool"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                self.tool_name,
                "toolset mock tool",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
    }

    fn make_tools_config_with_web_disabled() -> ironhermes_core::config::ToolsConfig {
        ironhermes_core::config::ToolsConfig::default() // web is disabled by default
    }

    fn make_tools_config_with_web_enabled() -> ironhermes_core::config::ToolsConfig {
        let mut cfg = ironhermes_core::config::ToolsConfig::default();
        cfg.toolsets.insert(
            "web".to_string(),
            ironhermes_core::config::ToolsetEntry { enabled: true },
        );
        cfg
    }

    /// Test: MockTool with toolset "web"; default ToolsConfig has web disabled;
    /// get_definitions returns empty. Enable web; returns the schema.
    #[test]
    fn set_toolset_config_then_get_definitions_filters_by_toolset() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ToolsetMockTool {
            tool_name: "web_mock",
            toolset_name: "web",
        }));

        // With web disabled (default)
        registry.set_toolset_config(Some(make_tools_config_with_web_disabled()));
        let defs = registry.get_definitions(None);
        let names: Vec<String> = defs.iter().map(|s| s.function.name.clone()).collect();
        assert!(
            !names.iter().any(|n| n == "web_mock"),
            "web_mock must be filtered out when web toolset is disabled; got: {:?}",
            names
        );

        // Enable web
        registry.set_toolset_config(Some(make_tools_config_with_web_enabled()));
        let defs = registry.get_definitions(None);
        let names: Vec<String> = defs.iter().map(|s| s.function.name.clone()).collect();
        assert!(
            names.iter().any(|n| n == "web_mock"),
            "web_mock must appear when web toolset is enabled; got: {:?}",
            names
        );
    }

    /// Test (D-A2 / Pitfall 8): toolset_config = None → get_definitions returns schema
    /// regardless of toolset name (preserves pre-Phase-25 behavior).
    #[test]
    fn get_definitions_no_config_preserves_existing_behavior() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ToolsetMockTool {
            tool_name: "any_tool",
            toolset_name: "some_weird_toolset",
        }));
        // No toolset_config set (None is the default)
        let defs = registry.get_definitions(None);
        let names: Vec<String> = defs.iter().map(|s| s.function.name.clone()).collect();
        assert!(
            names.iter().any(|n| n == "any_tool"),
            "With no toolset_config, any_tool must appear regardless of toolset; got: {:?}",
            names
        );
    }

    /// Regression (Phase 45 UAT #3): dynamic MCP tools (toolset() == MCP_TOOLSET) must
    /// survive the layer-1 toolset-enabled filter even though "mcp" is absent from the
    /// toolset taxonomy (ALL_TOOLSETS) and from any user config. Before the exemption,
    /// `is_toolset_enabled("mcp")` returned false → get_definitions silently hid every
    /// discovered MCP tool from the LLM, so a connected Cloudflare server exposed 0 tools
    /// to the agent. The exemption must be MCP-specific: a non-MCP tool whose toolset is
    /// not enabled is still filtered.
    #[test]
    fn get_definitions_exempts_mcp_toolset_from_filter() {
        let mut registry = ToolRegistry::new();
        // A discovered MCP tool (dynamic) reports the "mcp" toolset sentinel.
        registry.register_dynamic(Box::new(ToolsetMockTool {
            tool_name: "cloudflare_bindings__r2_bucket_delete",
            toolset_name: MCP_TOOLSET,
        }));
        // A non-MCP tool whose toolset is NOT enabled — proves the exemption is specific
        // to MCP and does not blanket-disable the toolset filter.
        registry.register(Box::new(ToolsetMockTool {
            tool_name: "unlisted_toolset_tool",
            toolset_name: "definitely_not_a_real_toolset",
        }));
        // A realistic toolset_config that does NOT list "mcp" (mirrors production configs).
        registry.set_toolset_config(Some(ironhermes_core::config::ToolsConfig::default()));

        let names: Vec<String> = registry
            .get_definitions(None)
            .iter()
            .map(|s| s.function.name.clone())
            .collect();

        assert!(
            names
                .iter()
                .any(|n| n == "cloudflare_bindings__r2_bucket_delete"),
            "MCP tool must survive the toolset filter (Phase 45 UAT #3 blocker); got: {:?}",
            names
        );
        assert!(
            !names.iter().any(|n| n == "unlisted_toolset_tool"),
            "non-MCP tool with an unlisted toolset must still be filtered; got: {:?}",
            names
        );
    }

    /// Regression (kanban image-task end-to-end follow-up): kanban worker-protocol tools
    /// (toolset() == KANBAN_TOOLSET) must survive the layer-1 toolset-enabled filter even
    /// though "kanban" is absent from ALL_TOOLSETS and every user config. Before the exemption,
    /// `is_toolset_enabled("kanban")` returned false → get_definitions silently hid
    /// kanban_complete/kanban_block from the worker's LLM, so the worker had no way to terminate
    /// its task (it ran `kanban_show` as a shell command and the task never completed). The
    /// exemption must be kanban-specific: a non-kanban tool with an unlisted toolset is still hidden.
    #[test]
    fn get_definitions_exempts_kanban_toolset_from_filter() {
        let mut registry = ToolRegistry::new();
        // A kanban worker-protocol tool reports the "kanban" toolset sentinel.
        registry.register(Box::new(ToolsetMockTool {
            tool_name: "kanban_complete",
            toolset_name: KANBAN_TOOLSET,
        }));
        // A non-kanban tool whose toolset is NOT enabled — proves the exemption is specific.
        registry.register(Box::new(ToolsetMockTool {
            tool_name: "unlisted_toolset_tool",
            toolset_name: "definitely_not_a_real_toolset",
        }));
        // A realistic toolset_config that does NOT list "kanban" (mirrors production configs).
        registry.set_toolset_config(Some(ironhermes_core::config::ToolsConfig::default()));

        let names: Vec<String> = registry
            .get_definitions(None)
            .iter()
            .map(|s| s.function.name.clone())
            .collect();

        assert!(
            names.iter().any(|n| n == "kanban_complete"),
            "kanban tool must survive the toolset filter so workers can terminate; got: {:?}",
            names
        );
        assert!(
            !names.iter().any(|n| n == "unlisted_toolset_tool"),
            "non-kanban tool with an unlisted toolset must still be filtered; got: {:?}",
            names
        );
    }

    /// Regression (D-09a, Phase 46): `scope_to()` must exempt dynamic MCP tools
    /// (toolset() == MCP_TOOLSET) from the caller-supplied toolset allowlist, mirroring
    /// the already-shipped get_definitions() exemption above. Before this fix, subagents
    /// and kanban workers that call scope_to() with an allowlist that (correctly) never
    /// lists "mcp" — because "mcp" is not part of the built-in toolset taxonomy — would
    /// get zero MCP tools even when a Cloudflare MCP server is connected.
    #[test]
    fn scope_to_exempts_mcp_toolset() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ToolsetMockTool {
            tool_name: "web_mock",
            toolset_name: "web",
        }));
        registry.register_dynamic(Box::new(ToolsetMockTool {
            tool_name: "cloudflare_bindings__r2_bucket_list",
            toolset_name: MCP_TOOLSET,
        }));

        // Allowlist omits both "web" and "mcp" — only "fs" is allowed.
        let scoped = registry.scope_to(&["fs".to_string()]);
        let names: Vec<String> = scoped.tools.keys().cloned().collect();

        assert!(
            names
                .iter()
                .any(|n| n == "cloudflare_bindings__r2_bucket_list"),
            "MCP tool must survive scope_to() regardless of the caller's toolset allowlist (D-09a); got: {:?}",
            names
        );
        assert!(
            !names.iter().any(|n| n == "web_mock"),
            "non-MCP tool whose toolset is not in the allowlist must still be dropped; got: {:?}",
            names
        );
    }

    /// Test (D-23 layer 4): per-tool disabled list excludes a specific tool within an enabled toolset.
    #[test]
    fn get_definitions_per_tool_disabled_filter() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ToolsetMockTool {
            tool_name: "regular",
            toolset_name: "memory", // memory is enabled by default
        }));
        let mut cfg = ironhermes_core::config::ToolsConfig::default();
        cfg.disabled.push("regular".to_string());
        registry.set_toolset_config(Some(cfg));

        let defs = registry.get_definitions(None);
        let names: Vec<String> = defs.iter().map(|s| s.function.name.clone()).collect();
        assert!(
            !names.iter().any(|n| n == "regular"),
            "Tool in disabled list must be excluded even when toolset is enabled; got: {:?}",
            names
        );
    }

    /// Test (D-23 intercept toolset mapping): intercepts filtered by owner toolset.
    #[test]
    fn get_definitions_intercepted_owner_toolset_mapping() {
        let mut registry = ToolRegistry::new();
        // Register 3 intercepts: memory (memory toolset), session_search (session), delegate_task (agent)
        for name in &["memory", "session_search", "delegate_task"] {
            registry.register_intercepted(name, test_intercept_schema(name), test_handler("ok"));
        }

        // Default config: memory+session+agent all enabled → all 3 present
        let cfg = ironhermes_core::config::ToolsConfig::default();
        registry.set_toolset_config(Some(cfg));
        let defs = registry.get_definitions(None);
        let names: std::collections::HashSet<String> =
            defs.iter().map(|s| s.function.name.clone()).collect();
        assert!(
            names.contains("memory"),
            "memory intercept must be present when memory toolset enabled"
        );
        assert!(
            names.contains("session_search"),
            "session_search must be present when session toolset enabled"
        );
        assert!(
            names.contains("delegate_task"),
            "delegate_task must be present when agent toolset enabled"
        );

        // Disable agent toolset → delegate_task disappears, others remain
        let mut cfg2 = ironhermes_core::config::ToolsConfig::default();
        cfg2.toolsets.insert(
            "agent".to_string(),
            ironhermes_core::config::ToolsetEntry { enabled: false },
        );
        registry.set_toolset_config(Some(cfg2));
        let defs2 = registry.get_definitions(None);
        let names2: std::collections::HashSet<String> =
            defs2.iter().map(|s| s.function.name.clone()).collect();
        assert!(
            !names2.contains("delegate_task"),
            "delegate_task must be absent when agent toolset disabled"
        );
        assert!(names2.contains("memory"), "memory must still be present");
        assert!(
            names2.contains("session_search"),
            "session_search must still be present"
        );
    }

    /// Test (D-15 collision guard): registering non-intercepted regular tool + intercepted names
    /// does NOT panic when name sets are disjoint — the binary's actual setup contract.
    #[test]
    fn with_intercepts_does_not_collide_with_regular_registration() {
        let mut registry = ToolRegistry::new();
        // Register a non-intercepted regular tool (like web_search in the binary)
        registry.register(Box::new(ToolsetMockTool {
            tool_name: "web_search",
            toolset_name: "web",
        }));
        // Register intercepted names (different from the regular tool's name)
        registry.register_intercepted("memory", test_intercept_schema("memory"), test_handler("m"));
        registry.register_intercepted(
            "delegate_task",
            test_intercept_schema("delegate_task"),
            test_handler("dt"),
        );
        registry.register_intercepted(
            "cronjob",
            test_intercept_schema("cronjob"),
            test_handler("cj"),
        );
        // Reaching here means no D-15 panic fired — the test passes by completing
        let defs = registry.get_definitions(None);
        let names: Vec<String> = defs.iter().map(|s| s.function.name.clone()).collect();
        assert!(
            names.contains(&"web_search".to_string()),
            "regular tool must appear"
        );
        assert!(
            names.contains(&"memory".to_string()),
            "memory intercept must appear"
        );
    }

    #[tokio::test]
    async fn test_guardrail_error_detail_minimal() {
        let mut registry = make_registry_with_tool("secret_tool");
        registry.set_error_detail(ironhermes_hooks::ErrorDetailLevel::Minimal);
        registry.add_guardrail(Box::new(BlocklistGuardrail::new(vec![
            "secret_tool".to_string(),
        ])));

        let result = registry
            .dispatch("secret_tool", serde_json::Value::Null)
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert_eq!(
            err_msg, "Tool call blocked by security policy",
            "minimal detail must not leak tool name: {err_msg}"
        );
        assert!(
            !err_msg.contains("secret_tool"),
            "tool name must not appear in minimal error: {err_msg}"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 25.1 D-04: register_browser_tools registers exactly 11 browser_* tools
    // -----------------------------------------------------------------------

    #[test]
    fn register_browser_tools_registers_all_11() {
        use ironhermes_core::{config::Config, provider::ProviderResolver};
        let mut registry = ToolRegistry::new();
        let session = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let config = Config::default();
        let resolver = std::sync::Arc::new(
            ProviderResolver::build(&config).expect("default config builds resolver"),
        );
        registry.register_browser_tools(session, resolver, std::sync::Arc::new(config));

        let names: std::collections::HashSet<String> = registry
            .list_tools()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        for expected in &[
            "browser_back",
            "browser_click",
            "browser_close",
            "browser_console",
            "browser_get_images",
            "browser_navigate",
            "browser_press",
            "browser_scroll",
            "browser_snapshot",
            "browser_type",
            "browser_vision",
        ] {
            assert!(
                names.contains(*expected),
                "Phase 25.1 D-04: tool {} MUST be registered (got: {:?})",
                expected,
                names
            );
        }
        let browser_count = names.iter().filter(|n| n.starts_with("browser_")).count();
        assert_eq!(
            browser_count, 11,
            "exactly 11 browser_* tools must be registered"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 25.1 GAP-5: diagnostic tests — browser_close vs browser_navigate
    // in get_definitions() output (the LLM-visible schema list).
    // Run with: cargo test -p ironhermes-tools --lib diagnose_gap_5 -- --nocapture
    // -----------------------------------------------------------------------

    /// Phase 25.1 GAP-5 diagnostic: prove browser_close appears in get_definitions
    /// alongside browser_navigate, and dump both schemas for inspection.
    /// Run with `cargo test -p ironhermes-tools diagnose_gap_5 -- --nocapture` to see schemas.
    #[test]
    fn diagnose_gap_5_browser_close_appears_in_get_definitions() {
        use ironhermes_core::{config::Config, provider::ProviderResolver};
        let mut registry = ToolRegistry::new();
        let session = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let config = Config::default();
        let resolver = std::sync::Arc::new(
            ProviderResolver::build(&config).expect("default config builds resolver"),
        );
        // Pass Arc<Config> — plan 14 signature (3 args).
        registry.register_browser_tools(session, resolver, std::sync::Arc::new(config));

        // No toolset_config set → get_definitions returns all tools where is_available()
        // passes. This is the same filter path as the LLM schema production in AgentLoop
        // when toolset_config is set with browser enabled (just the toolset layer is
        // bypassed here; we verify structural presence).
        let schemas = registry.get_definitions(None);

        let close = schemas.iter().find(|s| s.function.name == "browser_close");
        let navigate = schemas
            .iter()
            .find(|s| s.function.name == "browser_navigate");

        // Hypothesis 3 floor: if browser_close is missing here, the registry filter
        // (is_available / toolset / disabled) is excluding it. This MUST not be the case
        // because both share identical impls.
        assert!(
            close.is_some(),
            "GAP-5 hypothesis 3: browser_close MUST appear in get_definitions"
        );
        assert!(
            navigate.is_some(),
            "browser_navigate MUST appear in get_definitions"
        );

        let close = close.unwrap();
        let navigate = navigate.unwrap();

        // Diagnostic dump — captured to SUMMARY.md for the fix task to reference.
        println!("\n=== GAP-5 DIAGNOSTIC: browser_close schema ===");
        println!("{}", serde_json::to_string_pretty(&close).unwrap());
        println!("\n=== GAP-5 DIAGNOSTIC: browser_navigate schema (control) ===");
        println!("{}", serde_json::to_string_pretty(&navigate).unwrap());
        println!("\n=== GAP-5 DIAGNOSTIC: description comparison ===");
        println!("close:    {}", close.function.description);
        println!("navigate: {}", navigate.function.description);

        // Structural sanity (hypothesis 2 — schema malformation):
        assert!(
            close.function.description.len() >= 10,
            "browser_close description too short: {:?}",
            close.function.description
        );
        let params = &close.function.parameters;
        assert_eq!(
            params["type"], "object",
            "browser_close.schema parameters MUST be type:object"
        );
        assert!(
            params["properties"].is_object(),
            "browser_close.schema MUST have properties:{{}}"
        );
        assert!(
            params["required"].is_array(),
            "browser_close.schema MUST have required:[]"
        );
    }

    /// Phase 25.1 GAP-5 diagnostic: prove browser_close and browser_navigate share
    /// identical filter signals (toolset, is_available, prerequisites count).
    /// If this test passes, hypothesis 3 (path divergence) is FALSE.
    #[test]
    fn diagnose_gap_5_browser_close_and_navigate_have_isomorphic_filters() {
        use ironhermes_core::{config::Config, provider::ProviderResolver};
        let mut registry = ToolRegistry::new();
        let session = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let config = Config::default();
        let resolver = std::sync::Arc::new(
            ProviderResolver::build(&config).expect("default config builds resolver"),
        );
        registry.register_browser_tools(session, resolver, std::sync::Arc::new(config));

        // Access tools map directly (we are in the same module as the private field).
        let close = registry
            .tools
            .get("browser_close")
            .expect("browser_close registered");
        let navigate = registry
            .tools
            .get("browser_navigate")
            .expect("browser_navigate registered");

        assert_eq!(
            close.toolset(),
            navigate.toolset(),
            "GAP-5 hypothesis 3: toolset() MUST match. close={:?} navigate={:?}",
            close.toolset(),
            navigate.toolset()
        );
        assert_eq!(
            close.is_available(),
            navigate.is_available(),
            "GAP-5 hypothesis 3: is_available() MUST match for both tools"
        );
        assert_eq!(
            close.prerequisites().len(),
            navigate.prerequisites().len(),
            "GAP-5 hypothesis 3: prerequisites() length MUST match"
        );

        println!("\n=== GAP-5 DIAGNOSTIC: filter signals ===");
        println!(
            "close    toolset={:?}  is_available={}  prerequisites.len={}",
            close.toolset(),
            close.is_available(),
            close.prerequisites().len()
        );
        println!(
            "navigate toolset={:?}  is_available={}  prerequisites.len={}",
            navigate.toolset(),
            navigate.is_available(),
            navigate.prerequisites().len()
        );
    }

    // ---------------------------------------------------------------------------
    // Phase 27.1.1 gap-01: canonical entry-point regression tests
    //
    // These tests are the structural guard against tool-registration drift.
    // If a new tool is added to register_defaults_except() but bypassed by a
    // production entry point, the cross-check tests below catch it at CI time.
    // ---------------------------------------------------------------------------

    /// Regression test for HXP-TOOL-01 (Phase 27.1.1 UAT failure root cause).
    ///
    /// register_defaults() MUST include hexapod_tcp. The first UAT attempt failed
    /// because production paths bypassed register_defaults() and hand-rolled their
    /// own lists without hexapod_tcp. This test ensures the canonical entry point
    /// always includes it, so any path that delegates to register_defaults[_except]
    /// inherits hexapod_tcp automatically.
    #[test]
    fn test_register_defaults_includes_hexapod_tcp() {
        // HEXAPOD_IP may not be set in CI — we check tool registration, not availability.
        // get_definitions(None) returns ALL registered tools regardless of is_available().
        let mut registry = ToolRegistry::new();
        registry.register_defaults();
        let names = registry.list_tools();
        assert!(
            names.contains(&"hexapod_tcp"),
            "register_defaults() MUST register hexapod_tcp; \
             missing = tool-registration drift that caused Phase 27.1.1 UAT failure. \
             All tools registered: {:?}",
            names
        );
    }

    /// register_defaults() MUST include hexapod_video (Phase 27.1.4).
    #[test]
    fn test_register_defaults_includes_hexapod_video() {
        // HEXAPOD_IP may not be set in CI — we check tool registration, not availability.
        // list_tools() returns ALL registered tools regardless of is_available().
        let mut registry = ToolRegistry::new();
        registry.register_defaults();
        let names = registry.list_tools();
        assert!(
            names.contains(&"hexapod_video"),
            "register_defaults() MUST register hexapod_video; \
             all tools registered: {:?}",
            names
        );
    }

    /// register_defaults_except(&["terminal"]) MUST skip terminal and register everything else.
    ///
    /// This is the canonical call pattern for production paths that supply their own
    /// process-registry-wired terminal (app_runtime_factory, event_loop).
    #[test]
    fn test_register_defaults_except_terminal_skips_terminal_only() {
        let mut registry = ToolRegistry::new();
        registry.register_defaults_except(&["terminal"]);
        let names = registry.list_tools();

        assert!(
            !names.contains(&"terminal"),
            "register_defaults_except(&[\"terminal\"]) must NOT register the plain terminal; \
             got tools: {:?}",
            names
        );

        // All other defaults must still be present.
        // Note: PatchFileTool::name() returns "patch" (not "patch_file").
        for expected in &[
            "read_file",
            "write_file",
            "patch",
            "search_files",
            "web_search",
            "web_answer",
            "web_read",
            "hexapod_tcp",
            "hexapod_video",
        ] {
            assert!(
                names.contains(expected),
                "register_defaults_except(&[\"terminal\"]) must still register '{}'; \
                 got tools: {:?}",
                expected,
                names
            );
        }
    }

    /// D-07 (Phase 41.3 Plan 08): `register_defaults()` must register
    /// `web_answer` in the `web` toolset — the answer half of the D-07
    /// split, alongside `web_search` (results, Plan 07).
    #[test]
    fn web_answer_is_registered_in_default_tools() {
        let mut registry = ToolRegistry::new();
        registry.register_defaults();
        let tool = registry
            .get("web_answer")
            .expect("register_defaults() must register web_answer");
        assert_eq!(tool.toolset(), "web");
    }

    /// Mirrors `test_register_defaults_except_terminal_skips_terminal_only`'s
    /// skip-list semantics for the newly added tool.
    #[test]
    fn register_defaults_except_can_skip_web_answer() {
        let mut registry = ToolRegistry::new();
        registry.register_defaults_except(&["web_answer"]);
        let names = registry.list_tools();

        assert!(
            !names.contains(&"web_answer"),
            "register_defaults_except(&[\"web_answer\"]) must NOT register web_answer; \
             got tools: {:?}",
            names
        );
        for expected in &["web_search", "web_read", "terminal"] {
            assert!(
                names.contains(expected),
                "register_defaults_except(&[\"web_answer\"]) must still register '{}'; \
                 got tools: {:?}",
                expected,
                names
            );
        }
    }

    /// Phase 41.3 Plan 08 (D-07): pins the count of `toolset() == "web"`
    /// tools `register_defaults()` registers, so a future addition/removal
    /// is a deliberate, reviewed change — the protective intent behind
    /// CONTEXT.md's Deferred Ideas note that "the web toolset reaches six
    /// entries". That narrative figure counts the WIDER conceptual
    /// web-tool family across the whole crate (`web_extract`, `image_gen`,
    /// `video_generate`, `video_animate`, `video_to_video` are ALSO
    /// `toolset() == "web"`, verified via `grep -rn '"web"' src/*.rs`, but
    /// every one of them is registered separately via its own
    /// `register_*_tool` method requiring a runtime handle
    /// `register_defaults()` cannot construct) — none of which
    /// `register_defaults()` itself ever registers. Within
    /// `register_defaults()`'s actual scope the true, verified count is
    /// three: `web_search`, `web_answer`, `web_read`.
    #[test]
    fn web_toolset_tool_count_registered_by_defaults_is_pinned() {
        let mut registry = ToolRegistry::new();
        registry.register_defaults();
        let web_tools: Vec<&str> = registry
            .tools
            .values()
            .filter(|t| t.toolset() == "web")
            .map(|t| t.name())
            .collect();
        assert_eq!(
            web_tools.len(),
            3,
            "register_defaults() must register exactly 3 'web'-toolset tools \
             (web_search, web_answer, web_read); got: {:?}",
            web_tools
        );
    }

    /// register_defaults() and register_defaults_except(&[]) must produce the same tool set.
    ///
    /// Cross-check: if these diverge, one or more tool registrations in
    /// register_defaults_except are gated incorrectly.
    #[test]
    fn test_register_defaults_and_except_empty_produce_same_set() {
        let mut reg_a = ToolRegistry::new();
        reg_a.register_defaults();
        let mut names_a = reg_a.list_tools();
        names_a.sort();

        let mut reg_b = ToolRegistry::new();
        reg_b.register_defaults_except(&[]);
        let mut names_b = reg_b.list_tools();
        names_b.sort();

        assert_eq!(
            names_a, names_b,
            "register_defaults() and register_defaults_except(&[]) must register identical tool sets"
        );
    }

    // ---------------------------------------------------------------------
    // Phase 36.17.7 D-06 (REVISION BLOCKER 2 — Path B): TTS registration
    // status accessor — Live / Inspection / NotRegistered.
    // ---------------------------------------------------------------------

    /// Empty registry — `tts_registration_status` is `NotRegistered`,
    /// `is_tts_registered_live` is false.
    #[test]
    fn tts_registration_status_empty_registry_is_not_registered() {
        use ironhermes_core::commands::context::TtsRegistrationStatus;
        let reg = ToolRegistry::new();
        assert_eq!(
            reg.tts_registration_status(),
            TtsRegistrationStatus::NotRegistered,
            "empty registry must report NotRegistered"
        );
        assert!(
            !reg.is_tts_registered_live(),
            "empty registry must not be live"
        );
    }

    /// Sentinel SessionKey (Platform::Local, "inspect") → Inspection.
    #[test]
    fn tts_registration_status_inspection_sentinel() {
        use ironhermes_core::commands::context::TtsRegistrationStatus;
        use ironhermes_core::{Platform, SessionKey};
        let mut reg = ToolRegistry::new();
        let key = SessionKey::new(Platform::Local, "inspect");
        reg.register_tts_tools(
            key,
            None,
            std::sync::Arc::new(ironhermes_core::Config::default()),
        );
        assert_eq!(
            reg.tts_registration_status(),
            TtsRegistrationStatus::Inspection,
            "sentinel session_key must report Inspection"
        );
        assert!(
            !reg.is_tts_registered_live(),
            "sentinel session_key must NOT be live"
        );
    }

    /// Real session_key (Platform::Web, "session-uuid") → Live.
    #[test]
    fn tts_registration_status_live_for_real_session_key() {
        use ironhermes_core::commands::context::TtsRegistrationStatus;
        use ironhermes_core::{Platform, SessionKey};
        let mut reg = ToolRegistry::new();
        let key = SessionKey::new(Platform::Web, "session-abc-123");
        reg.register_tts_tools(
            key,
            None,
            std::sync::Arc::new(ironhermes_core::Config::default()),
        );
        assert_eq!(
            reg.tts_registration_status(),
            TtsRegistrationStatus::Live,
            "real (Web, ..) session_key must report Live"
        );
        assert!(
            reg.is_tts_registered_live(),
            "real (Web, ..) session_key must be live"
        );
    }

    // ---------------------------------------------------------------------------
    // Skill-as-tool fallback: a bare skill-name tool call (e.g. "arxiv") routes
    // to the `skills` tool with action=activate instead of erroring "Tool not
    // found". Rescues weaker local models that invent a tool named after a skill.
    // ---------------------------------------------------------------------------

    fn skill_md(name: &str, description: &str, body: &str) -> String {
        format!(
            "---\nname: {}\ndescription: {}\n---\n{}",
            name, description, body
        )
    }

    /// Build a ToolRegistry whose `skills` tool (and skill-as-tool handle) is
    /// backed by a temp dir containing the given `(name, description, body)` skills.
    /// Returns the registry plus the TempDir guard (kept alive for the test).
    fn registry_with_skills(skills: &[(&str, &str, &str)]) -> (ToolRegistry, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        for (name, description, body) in skills {
            let skill_dir = skills_dir.join(name);
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                skill_md(name, description, body),
            )
            .unwrap();
        }
        let skill_registry =
            std::sync::Arc::new(ironhermes_core::SkillRegistry::load_with_paths(&[
                skills_dir,
            ]));
        let active_skills = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let cred_dir = tempfile::tempdir().unwrap().keep();
        let mut registry = ToolRegistry::new();
        registry.register_skills_tool(
            skill_registry,
            active_skills,
            cred_dir,
            std::collections::HashMap::new(),
        );
        (registry, dir)
    }

    /// A real tool literally named "arxiv" — used to prove a registered tool
    /// wins over the skill-name fallback (the fallback only fires on a miss).
    struct FakeArxivTool;

    #[async_trait]
    impl Tool for FakeArxivTool {
        fn name(&self) -> &str {
            "arxiv"
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "a real tool that shadows the arxiv skill name"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "arxiv",
                "a real tool that shadows the arxiv skill name",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("REAL_TOOL".to_string())
        }
    }

    #[tokio::test]
    async fn bare_skill_name_call_routes_to_skills_activate() {
        let (registry, _guard) =
            registry_with_skills(&[("arxiv", "search arxiv papers", "ARXIV_SKILL_BODY")]);
        let out = registry
            .execute_tool("arxiv", serde_json::json!({}))
            .await
            .expect("bare skill-name call should resolve via the skills-tool fallback");
        let v: serde_json::Value = serde_json::from_str(&out).expect("skills tool returns JSON");
        assert_eq!(v["status"], "ok", "activate must succeed; got: {out}");
        assert_eq!(v["name"], "arxiv");
        assert!(
            v["content"]
                .as_str()
                .unwrap_or_default()
                .contains("ARXIV_SKILL_BODY"),
            "activate must return the SKILL.md body; got: {out}"
        );
    }

    #[tokio::test]
    async fn bare_skill_name_fallback_is_case_insensitive() {
        let (registry, _guard) =
            registry_with_skills(&[("arxiv", "search arxiv papers", "ARXIV_SKILL_BODY")]);
        // SkillRegistry::find lowercases both sides, so "ARXIV" must resolve too.
        let out = registry
            .execute_tool("ARXIV", serde_json::json!({}))
            .await
            .expect("case-insensitive skill name should resolve");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["status"], "ok", "got: {out}");
        assert_eq!(v["name"], "arxiv", "canonical skill name is returned");
    }

    #[tokio::test]
    async fn genuinely_unknown_tool_still_errors() {
        let (registry, _guard) =
            registry_with_skills(&[("arxiv", "search arxiv papers", "ARXIV_SKILL_BODY")]);
        let err = registry
            .execute_tool("definitely_not_a_tool_or_skill", serde_json::json!({}))
            .await
            .expect_err("a name that is neither tool nor skill must still error");
        assert!(
            err.to_string().contains("Tool not found"),
            "expected 'Tool not found' error; got: {err}"
        );
    }

    #[tokio::test]
    async fn real_tool_wins_over_skill_name() {
        // Register a real tool named "arxiv" AND have an "arxiv" skill present.
        // The fallback only fires on a tools-map miss, so the real tool must win.
        let (mut registry, _guard) =
            registry_with_skills(&[("arxiv", "search arxiv papers", "ARXIV_SKILL_BODY")]);
        registry.register(Box::new(FakeArxivTool));
        let out = registry
            .execute_tool("arxiv", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(
            out, "REAL_TOOL",
            "a registered tool must take precedence over the skill-name fallback"
        );
    }

    #[tokio::test]
    async fn fallback_noop_when_no_skills_registered() {
        // No register_skills_tool() call → skill_registry is None → fallback is a
        // no-op and the normal error path is preserved.
        let registry = ToolRegistry::new();
        let err = registry
            .execute_tool("arxiv", serde_json::json!({}))
            .await
            .expect_err("with no skills registered, a skill name must error normally");
        assert!(err.to_string().contains("Tool not found"), "got: {err}");
    }

    // -----------------------------------------------------------------------
    // Phase 46.6 Plan 02: `artifact` tool registration + toolset visibility
    // -----------------------------------------------------------------------

    #[test]
    fn artifact_tool_registers_in_registry() {
        use ironhermes_core::config::Config;
        let mut registry = ToolRegistry::new();
        registry.register_artifact_tool(Arc::new(Config::default()));
        let tools = registry.list_tools();
        assert!(
            tools.contains(&"artifact"),
            "register_artifact_tool must register a tool named 'artifact'; got: {tools:?}"
        );
    }

    /// Regression guard (RESEARCH Pitfall 4): the `artifact` tool must survive the
    /// layer-1 toolset-enabled filter for chat, delegate, AND kanban-worker toolset
    /// configs because `"artifacts"` is a normal `ALL_TOOLSETS` member — it must
    /// NOT rely on the `MCP_TOOLSET`/`KANBAN_TOOLSET` exemption branch.
    #[test]
    fn artifact_toolset_visible_all_surfaces() {
        use ironhermes_core::config::{Config, ToolsConfig};

        let build_registry = || {
            let mut registry = ToolRegistry::new();
            registry.register_artifact_tool(Arc::new(Config::default()));
            registry
        };

        // Chat / default toolset config.
        let mut chat_registry = build_registry();
        chat_registry
            .set_toolset_config(Some(ToolsConfig::default().with_default_toolsets_merged()));
        let chat_names: Vec<String> = chat_registry
            .get_definitions(None)
            .iter()
            .map(|s| s.function.name.clone())
            .collect();
        assert!(
            chat_names.iter().any(|n| n == "artifact"),
            "artifact tool must be visible under a chat/default toolset config; got: {chat_names:?}"
        );

        // Delegate-style toolset config: only a subset of toolsets enabled via
        // enabled_tools narrowing (D-23 layer 3) — "artifacts" must still pass
        // layer 1 because it's an ALL_TOOLSETS member, independent of any
        // exemption path.
        let mut delegate_registry = build_registry();
        delegate_registry
            .set_toolset_config(Some(ToolsConfig::default().with_default_toolsets_merged()));
        let delegate_names: Vec<String> = delegate_registry
            .get_definitions(Some(&["artifact".to_string()]))
            .iter()
            .map(|s| s.function.name.clone())
            .collect();
        assert!(
            delegate_names.iter().any(|n| n == "artifact"),
            "artifact tool must be visible under a delegate-style toolset config; got: {delegate_names:?}"
        );

        // Kanban-worker toolset config: mirrors a realistic worker config that does
        // NOT list "kanban" (proving artifact visibility is independent of the
        // KANBAN_TOOLSET exemption path).
        let mut kanban_registry = build_registry();
        kanban_registry
            .set_toolset_config(Some(ToolsConfig::default().with_default_toolsets_merged()));
        let kanban_names: Vec<String> = kanban_registry
            .get_definitions(None)
            .iter()
            .map(|s| s.function.name.clone())
            .collect();
        assert!(
            kanban_names.iter().any(|n| n == "artifact"),
            "artifact tool must be visible under a kanban-worker toolset config; got: {kanban_names:?}"
        );
    }

    // ---------------------------------------------------------------------------
    // Phase 41.3 Plan 01 Task 2 (D-06): resolve_tool_timeout precedence-chain
    // unit tests. Pure — construct ToolsConfig values directly, never read
    // process env or write a config file, so these are parallel-safe.
    // ---------------------------------------------------------------------------
    mod timeout_precedence_tests {
        use super::*;

        /// Declares a fixed 120s budget.
        struct DeclaringTool;

        #[async_trait]
        impl Tool for DeclaringTool {
            fn name(&self) -> &str {
                "declaring"
            }
            fn toolset(&self) -> &str {
                "test"
            }
            fn description(&self) -> &str {
                "tool declaring a fixed timeout_secs"
            }
            fn schema(&self) -> ToolSchema {
                ToolSchema::new(
                    "declaring",
                    self.description(),
                    serde_json::json!({ "type": "object", "properties": {} }),
                )
            }
            fn timeout_secs(&self) -> Option<u64> {
                Some(120)
            }
            async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
                Ok(String::new())
            }
        }

        /// Opts out of the code-level bound via `None` (D-05's explicit,
        /// greppable opt-out line).
        struct OptedOutTool;

        #[async_trait]
        impl Tool for OptedOutTool {
            fn name(&self) -> &str {
                "opted_out"
            }
            fn toolset(&self) -> &str {
                "test"
            }
            fn description(&self) -> &str {
                "tool opting out of the code-level bound"
            }
            fn schema(&self) -> ToolSchema {
                ToolSchema::new(
                    "opted_out",
                    self.description(),
                    serde_json::json!({ "type": "object", "properties": {} }),
                )
            }
            fn timeout_secs(&self) -> Option<u64> {
                None
            }
            async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
                Ok(String::new())
            }
        }

        /// Does not override `timeout_secs()` at all — inherits the trait
        /// default (`Some(DEFAULT_TOOL_TIMEOUT_SECS)`).
        struct SilentTool;

        #[async_trait]
        impl Tool for SilentTool {
            fn name(&self) -> &str {
                "silent"
            }
            fn toolset(&self) -> &str {
                "test"
            }
            fn description(&self) -> &str {
                "tool that does not override timeout_secs()"
            }
            fn schema(&self) -> ToolSchema {
                ToolSchema::new(
                    "silent",
                    self.description(),
                    serde_json::json!({ "type": "object", "properties": {} }),
                )
            }
            async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
                Ok(String::new())
            }
        }

        fn cfg_with_global_default(secs: u64) -> ironhermes_core::config::ToolsConfig {
            ironhermes_core::config::ToolsConfig {
                timeout_secs: secs,
                ..Default::default()
            }
        }

        fn cfg_with_override(name: &str, secs: i64) -> ironhermes_core::config::ToolsConfig {
            let mut cfg = ironhermes_core::config::ToolsConfig::default();
            cfg.timeout_overrides.insert(name.to_string(), secs);
            cfg
        }

        /// D-06 level 1: an operator override wins even over a code-level
        /// `None` opt-out.
        #[test]
        fn override_beats_a_code_level_none() {
            let tool = OptedOutTool;
            let cfg = cfg_with_override("opted_out", 5);
            let budget = resolve_tool_timeout(&tool, "opted_out", &cfg);
            assert_eq!(budget, Some(Duration::from_secs(5)));
        }

        /// D-06 level 1: an operator override wins over a tool's own declared
        /// budget.
        #[test]
        fn override_beats_a_declaring_tool() {
            let tool = DeclaringTool;
            let cfg = cfg_with_override("declaring", 7);
            let budget = resolve_tool_timeout(&tool, "declaring", &cfg);
            assert_eq!(budget, Some(Duration::from_secs(7)));
        }

        /// D-06: a configured override of exactly `0` disables the bound.
        #[test]
        fn zero_override_disables() {
            let tool = DeclaringTool;
            let cfg = cfg_with_override("declaring", 0);
            let budget = resolve_tool_timeout(&tool, "declaring", &cfg);
            assert_eq!(budget, None);
        }

        /// D-06: a configured negative override also disables the bound.
        #[test]
        fn negative_override_disables() {
            let tool = DeclaringTool;
            let cfg = cfg_with_override("declaring", -1);
            let budget = resolve_tool_timeout(&tool, "declaring", &cfg);
            assert_eq!(budget, None);
        }

        /// D-06 level 2: a tool's own declared budget beats the operator's
        /// global default.
        #[test]
        fn declared_beats_global_default() {
            let tool = DeclaringTool;
            let cfg = cfg_with_global_default(45);
            let budget = resolve_tool_timeout(&tool, "declaring", &cfg);
            assert_eq!(budget, Some(Duration::from_secs(120)));
        }

        /// D-06 level 3: a tool that did not declare (trait default) picks up
        /// the operator's global default.
        #[test]
        fn global_default_applies_to_a_silent_tool() {
            let tool = SilentTool;
            let cfg = cfg_with_global_default(45);
            let budget = resolve_tool_timeout(&tool, "silent", &cfg);
            assert_eq!(
                budget,
                Some(Duration::from_secs(45)),
                "if this fails, the level-3 mechanism in resolve_tool_timeout is wrong \
                 — fix resolve_tool_timeout, do not relax this test"
            );
        }

        /// D-06 level 4: with a default `ToolsConfig` (global default itself
        /// unconfigured), a silent tool resolves to the trait-default floor.
        #[test]
        fn trait_default_is_the_floor() {
            let tool = SilentTool;
            let cfg = ironhermes_core::config::ToolsConfig::default();
            let budget = resolve_tool_timeout(&tool, "silent", &cfg);
            assert_eq!(budget, Some(Duration::from_secs(DEFAULT_TOOL_TIMEOUT_SECS)));
        }

        /// D-05: a code-level `None` opt-out is honored when unoverridden —
        /// the executable proof of D-05's inversion (omission yields a bound;
        /// opting out is one explicit, reviewable line).
        #[test]
        fn code_level_none_opts_out_when_unoverridden() {
            let tool = OptedOutTool;
            let cfg = ironhermes_core::config::ToolsConfig::default();
            let budget = resolve_tool_timeout(&tool, "opted_out", &cfg);
            assert_eq!(budget, None);
        }
    }

    // ---------------------------------------------------------------------------
    // Phase 48.2 Plan 01: display_group / catalog_rows regression tests
    // ---------------------------------------------------------------------------

    /// Tool with a required env_var prerequisite that catalog_rows() must
    /// surface even though it is unavailable (D-16 — the unfiltered read).
    struct CatalogUnmetPrereqTool;

    #[async_trait]
    impl Tool for CatalogUnmetPrereqTool {
        fn name(&self) -> &str {
            "catalog_unmet_prereq"
        }
        fn toolset(&self) -> &str {
            "test_catalog"
        }
        fn description(&self) -> &str {
            "catalog test tool with an unmet required prerequisite"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "catalog_unmet_prereq",
                self.description(),
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
        fn prerequisites(&self) -> Vec<Prerequisite> {
            vec![Prerequisite::env_var(
                "CATALOG_TEST_48_2_MISSING",
                "test-only missing env var",
                true,
            )]
        }
    }

    /// Test (a): catalog_rows() contains a tool whose required prerequisite
    /// is unmet — the unfiltered read must not hide it (D-16).
    #[test]
    fn catalog_rows_includes_tool_with_unmet_required_prerequisite() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::remove_var("CATALOG_TEST_48_2_MISSING") };
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(CatalogUnmetPrereqTool));
        let rows = registry.catalog_rows();
        let row = rows
            .iter()
            .find(|r| r.name == "catalog_unmet_prereq")
            .expect("catalog_rows() must include a tool with an unmet required prerequisite");
        assert!(!row.available, "row must report available: false");
        assert!(
            row.missing_prerequisites
                .iter()
                .any(|p| p.name == "CATALOG_TEST_48_2_MISSING"),
            "missing_prerequisites must name the unmet env var; got: {:?}",
            row.missing_prerequisites
        );
    }

    /// Tool reporting the MCP_TOOLSET sentinel from `toolset()` but a distinct
    /// `display_group()` override — proves catalog_rows()'s `group` field
    /// comes from display_group() while `toolset` stays exactly what
    /// Tool::toolset() returned (the D-20 axis-separation invariant).
    struct CatalogDisplayGroupTool {
        group: String,
    }

    #[async_trait]
    impl Tool for CatalogDisplayGroupTool {
        fn name(&self) -> &str {
            "catalog_display_group_tool"
        }
        fn toolset(&self) -> &str {
            MCP_TOOLSET
        }
        fn description(&self) -> &str {
            "catalog test tool with a display_group override"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                "catalog_display_group_tool",
                self.description(),
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
        fn display_group(&self) -> Option<&str> {
            Some(&self.group)
        }
    }

    /// Test (b): a tool whose display_group() is Some(g) emits group == g in
    /// catalog_rows() while `toolset` stays the value Tool::toolset() returned.
    #[test]
    fn catalog_rows_uses_display_group_for_group_but_keeps_real_toolset() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic(Box::new(CatalogDisplayGroupTool {
            group: "mcp__testserver".to_string(),
        }));
        let rows = registry.catalog_rows();
        let row = rows
            .iter()
            .find(|r| r.name == "catalog_display_group_tool")
            .expect("catalog_rows() must include the display_group tool");
        assert_eq!(row.group, "mcp__testserver");
        assert_eq!(row.toolset, MCP_TOOLSET);
    }

    /// Test (c): the Phase 45 guard — a registered tool reporting MCP_TOOLSET
    /// is still returned by get_definitions() when toolset_config is Some(cfg)
    /// and cfg has no "mcp" entry (the exemption is intact, unmodified by this
    /// plan's display_group addition).
    #[test]
    fn phase_45_guard_mcp_toolset_survives_get_definitions_with_no_mcp_config_entry() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic(Box::new(ToolsetMockTool {
            tool_name: "phase_45_guard_tool",
            toolset_name: MCP_TOOLSET,
        }));
        registry.set_toolset_config(Some(ironhermes_core::config::ToolsConfig::default()));
        let names: Vec<String> = registry
            .get_definitions(None)
            .iter()
            .map(|s| s.function.name.clone())
            .collect();
        assert!(
            names.iter().any(|n| n == "phase_45_guard_tool"),
            "Phase 45 exemption must survive this plan's changes; got: {:?}",
            names
        );
    }

    /// Test (d): the D-20 guard — the same MCP-toolset tool IS excluded by
    /// get_definitions() when its name is in cfg.disabled, proving the
    /// per-tool override reaches MCP tools through filter layer 4.
    #[test]
    fn d20_guard_mcp_toolset_tool_excluded_when_per_tool_disabled() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic(Box::new(ToolsetMockTool {
            tool_name: "d20_guard_tool",
            toolset_name: MCP_TOOLSET,
        }));
        let mut cfg = ironhermes_core::config::ToolsConfig::default();
        cfg.disabled.push("d20_guard_tool".to_string());
        registry.set_toolset_config(Some(cfg));
        let names: Vec<String> = registry
            .get_definitions(None)
            .iter()
            .map(|s| s.function.name.clone())
            .collect();
        assert!(
            !names.iter().any(|n| n == "d20_guard_tool"),
            "per-tool disabled list must reach MCP tools (D-20); got: {:?}",
            names
        );
    }
}
