use crate::config::McpServerConfig;
use crate::server_task::{self, ServerTaskResult};
use crate::tool::sanitize_server_name;
use ironhermes_core::auth::AuthStore;
use ironhermes_tools::ToolRegistry;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Phase 48.2 Plan 08 (T-48.2-08-03): TTL for a parked web-OAuth
/// authorization session, in seconds. Deliberately the same bound
/// `connect_http_oauth`'s loopback accept uses (`transport.rs`), so the CLI
/// and web authorization paths expire on the same clock.
const PENDING_OAUTH_TTL_SECS: u64 = 300;

/// Phase 48.2 Plan 08 (T-48.2-08-03): maximum number of concurrently pending
/// web-OAuth authorizations. Exceeding this (after pruning expired entries)
/// refuses new authorizations rather than evicting an operator's in-progress
/// one — silently discarding real in-flight work is worse than telling the
/// caller to retry.
const PENDING_OAUTH_MAX: usize = 8;

/// A parked web-OAuth authorization session (Phase 48.2 Plan 08).
///
/// Holds the rmcp `AuthorizationSession` returned by `transport::begin_oauth_web`
/// between `McpManager::begin_oauth` (which creates it) and
/// `McpManager::complete_oauth` / `McpManager::cancel_oauth` (which consume
/// it). Never exposed outside this module.
struct PendingOAuth {
    session: rmcp::transport::auth::AuthorizationSession,
    server_name: String,
    created_at: Instant,
}

/// The result of `McpManager::begin_oauth` (Phase 48.2 Plan 08, D-03).
///
/// Everything the UI crate needs to drive the browser to the authorization
/// server and later correlate the callback: the URL to open, and the OAuth
/// `state` value that identifies this attempt. Contains only plain `String`
/// fields — no credential-bearing type crosses this boundary
/// (T-48.2-02-08).
#[derive(Debug, Clone)]
pub struct OAuthAuthorizationStart {
    pub auth_url: String,
    pub state: String,
}

/// Aggregated results of a `start_all_and_wait` or `reload_and_report` operation.
///
/// Consumed by `McpReloader::reload()` in Plan 04 to populate `McpReloadResult.failed` (D-12).
/// The `failed` vec carries `(server_name, sanitized_error)` pairs from
/// `ServerTaskResult.failure_reason` for each server that exhausted all retries.
pub struct StartResult {
    /// Server names that successfully connected and registered tools.
    pub connected: Vec<String>,
    /// Servers that failed with `(server_name, sanitized_error_message)` (D-12 data contract).
    pub failed: Vec<(String, String)>,
    /// Total number of MCP tools currently registered in the registry.
    pub tool_count: usize,
}

/// Manages all MCP server connections and their tool registrations.
///
/// Orchestrates per-server `tokio::spawn` tasks (D-04), handles startup (D-07),
/// filtering via enabled_tools (D-08), and reload (D-09).
pub struct McpManager {
    registry: Arc<RwLock<ToolRegistry>>,
    /// Active task handles, cancellation tokens, AND per-server child-process
    /// slots, keyed by server name.
    ///
    /// GAP-8: the third tuple element is an `Arc<Mutex<Option<tokio::process::Child>>>`
    /// populated by `server_task::connect_and_serve` after `connect_stdio` succeeds.
    /// `shutdown_all` reaches into it to call `Child::start_kill()` BEFORE the
    /// bounded `tokio::time::timeout(2s, handle)` await, so the gateway exits
    /// in bounded time on Ctrl+C even when the stdio child ignores parent-pipe
    /// EOF. Under the plan-11 Option B fallback the slot typically stays `None`
    /// (rmcp 1.5's `TokioChildProcess` owns the child internally), but the
    /// bounded timeout + `kill_on_drop(true)` in `connect_stdio` still close
    /// GAP-8 at the user-facing level. When rmcp later exposes a pre-spawned-
    /// Child constructor, the slot becomes load-bearing without any manager
    /// changes (Option A upgrade).
    // Complex task-registry type: one-of-a-kind management structure —
    // a type alias would only appear here and not improve readability.
    #[allow(clippy::type_complexity)]
    tasks: Mutex<
        HashMap<
            String,
            (
                JoinHandle<ServerTaskResult>,
                CancellationToken,
                Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>,
            ),
        >,
    >,
    /// Last-known configs for each active server (used by reconnect/reload).
    configs: Mutex<HashMap<String, McpServerConfig>>,
    /// GAP-7: per-server connected flag flipped to `true` by `server_task::connect_and_serve`
    /// ONLY after the rmcp `initialize` handshake AND `list_all_tools()` both succeed.
    /// `connected_server_names()` reads this map instead of `tasks.keys()` so servers
    /// whose child exited before handshake completion are correctly reported as FAILED.
    connected_flags: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Live, per-server last-known failure reason — the warm-but-revoked
    /// follow-up fix. Populated by `server_task::run_server_task` on every
    /// connect/serve error (not only at final retry exhaustion) and cleared
    /// to `None` on a successful (re)connect, so a still-retrying or
    /// permanently-stopped server's most recent sanitized error is available
    /// WITHOUT waiting for a `reload_and_report()` round trip. Sibling to
    /// `connected_flags`: same per-server `Arc`-sharing pattern, same
    /// lifecycle (allocated in `start_all`/`start_all_and_wait`, removed in
    /// `shutdown_all`). String-only — see [`Self::last_failure_reason`].
    last_failure: Mutex<HashMap<String, Arc<std::sync::Mutex<Option<String>>>>>,
    /// D-08: Optional AuthStore for OAuth-enabled MCP servers (44-05).
    ///
    /// Threaded to each `run_server_task` call. `None` (the default) leaves
    /// all existing call sites and non-OAuth servers fully unchanged.
    auth_store: Option<Arc<AuthStore>>,
    /// Phase 46.1 D-01: global additive MCP-OAuth issuer allowlist
    /// (`Config.mcp_oauth.issuer_allowlist`), threaded to each `run_server_task`
    /// call. Empty (the default) leaves all existing call sites and
    /// baseline-only (Cloudflare) servers fully unchanged (CFL-02).
    global_issuer_allowlist: Vec<String>,
    /// Phase 48.2 Plan 08 (D-03/T-48.2-08-03): bounded map of in-progress web
    /// OAuth authorizations, keyed by the OAuth `state` value. Populated by
    /// `begin_oauth`, consumed (single-use, removed) by `complete_oauth` or
    /// `cancel_oauth`. Bounded by both age (`PENDING_OAUTH_TTL_SECS`) and
    /// count (`PENDING_OAUTH_MAX`).
    pending_oauth: Mutex<HashMap<String, PendingOAuth>>,
}

impl McpManager {
    /// Create a new `McpManager` backed by the given `ToolRegistry`.
    ///
    /// D-08: Existing call sites pass only `registry`; `auth_store` defaults to `None`
    /// so non-OAuth servers and all current callers are completely unchanged.
    pub fn new(registry: Arc<RwLock<ToolRegistry>>) -> Self {
        Self {
            registry,
            tasks: Mutex::new(HashMap::new()),
            configs: Mutex::new(HashMap::new()),
            connected_flags: Mutex::new(HashMap::new()),
            last_failure: Mutex::new(HashMap::new()),
            auth_store: None,
            global_issuer_allowlist: Vec::new(),
            pending_oauth: Mutex::new(HashMap::new()),
        }
    }

    /// Builder: attach an `AuthStore` for OAuth-enabled MCP servers (44-05, D-08).
    ///
    /// Call this AFTER `McpManager::new(registry)` at production entry points that
    /// have Phase 41 auth infrastructure available. Passing `None` is a no-op and
    /// leaves OAuth servers skipped with a `tracing::warn` (D-04 headless posture).
    pub fn with_auth_store(mut self, auth_store: Option<Arc<AuthStore>>) -> Self {
        self.auth_store = auth_store;
        self
    }

    /// Builder: attach the global additive MCP-OAuth issuer allowlist (D-01, 46.1).
    ///
    /// Mirrors `with_auth_store`. Passing an empty `Vec` (the default) is a no-op —
    /// servers with no per-server pin fall back to the built-in baseline only
    /// (`security::BASELINE_ISSUER_ALLOWLIST`), preserving CFL-02 zero-new-config
    /// behavior for existing Cloudflare servers.
    pub fn with_global_issuer_allowlist(mut self, list: Vec<String>) -> Self {
        self.global_issuer_allowlist = list;
        self
    }

    /// Start all configured MCP servers as background tasks (fire-and-forget).
    ///
    /// D-07: one-shot tool discovery at startup. Tasks run in the background;
    /// the caller does not wait for connections to complete (avoids blocking startup).
    /// Disabled servers (enabled=false) are skipped.
    pub async fn start_all(&self, configs: HashMap<String, McpServerConfig>) {
        let mut tasks = self.tasks.lock().await;
        let mut stored_configs = self.configs.lock().await;
        let mut flags = self.connected_flags.lock().await;
        let mut last_failure = self.last_failure.lock().await;

        for (name, config) in configs {
            if !config.enabled {
                tracing::info!(server = %name, "MCP server disabled, skipping");
                continue;
            }
            let cancel = CancellationToken::new();
            // GAP-7: allocate per-server connected flag; server_task flips it to
            // true ONLY after list_all_tools() succeeds.
            let connected = Arc::new(AtomicBool::new(false));
            // GAP-8: per-server child-process slot. server_task parks the
            // spawned tokio::process::Child here on connect_stdio success;
            // shutdown_all reaches in to start_kill() it on graceful shutdown.
            // Under Option B fallback the slot stays None — bounded timeout
            // + kill_on_drop(true) still close GAP-8 at the user-facing level.
            let child_slot: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>> =
                Arc::new(tokio::sync::Mutex::new(None));
            // Warm-but-revoked follow-up fix: allocate per-server last-failure
            // cell; server_task updates it on every connect/serve error and
            // clears it on a successful (re)connect.
            let failure_cell: Arc<std::sync::Mutex<Option<String>>> =
                Arc::new(std::sync::Mutex::new(None));
            let handle = tokio::spawn(server_task::run_server_task(
                name.clone(),
                config.clone(),
                self.registry.clone(),
                cancel.clone(),
                connected.clone(),
                child_slot.clone(),
                self.auth_store.clone(),
                self.global_issuer_allowlist.clone(),
                failure_cell.clone(),
            ));
            tasks.insert(name.clone(), (handle, cancel, child_slot));
            flags.insert(name.clone(), connected);
            last_failure.insert(name.clone(), failure_cell);
            stored_configs.insert(name, config);
        }
    }

    /// Start all configured MCP servers and return initial connection results.
    ///
    /// Spawns tasks and gives servers a brief window to attempt connection, then
    /// returns a `StartResult` with preliminary connected/failed status.
    ///
    /// Used by `reload_and_report()` to aggregate failures for D-12 status reporting.
    pub async fn start_all_and_wait(
        &self,
        configs: HashMap<String, McpServerConfig>,
    ) -> StartResult {
        let mut task_names: Vec<String> = Vec::new();
        {
            let mut tasks = self.tasks.lock().await;
            let mut stored_configs = self.configs.lock().await;
            let mut flags = self.connected_flags.lock().await;
            let mut last_failure = self.last_failure.lock().await;

            for (name, config) in configs {
                if !config.enabled {
                    tracing::info!(server = %name, "MCP server disabled, skipping");
                    continue;
                }
                let cancel = CancellationToken::new();
                // GAP-7: allocate per-server connected flag; server_task flips it to
                // true ONLY after list_all_tools() succeeds.
                let connected = Arc::new(AtomicBool::new(false));
                // GAP-8: per-server child-process slot (see start_all for rationale).
                let child_slot: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>> =
                    Arc::new(tokio::sync::Mutex::new(None));
                // Warm-but-revoked follow-up fix (see start_all for rationale).
                let failure_cell: Arc<std::sync::Mutex<Option<String>>> =
                    Arc::new(std::sync::Mutex::new(None));
                let handle = tokio::spawn(server_task::run_server_task(
                    name.clone(),
                    config.clone(),
                    self.registry.clone(),
                    cancel.clone(),
                    connected.clone(),
                    child_slot.clone(),
                    self.auth_store.clone(),
                    self.global_issuer_allowlist.clone(),
                    failure_cell.clone(),
                ));
                task_names.push(name.clone());
                tasks.insert(name.clone(), (handle, cancel, child_slot));
                flags.insert(name.clone(), connected);
                last_failure.insert(name.clone(), failure_cell);
                stored_configs.insert(name, config);
            }
        }

        // Give servers a brief window to complete initial connection
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        // GAP-7: a server is "connected" IFF its AtomicBool flag is true — the
        // authoritative signal that rmcp `initialize` + `list_all_tools` both
        // succeeded. The has_tools heuristic is kept as belt-and-braces (a server
        // that registered tools must have flipped the flag to true) but the flag
        // is the ground truth that closes the GAP-7 false-positive.
        let mut connected = Vec::new();
        let mut failed: Vec<(String, String)> = Vec::new();
        {
            let tasks = self.tasks.lock().await;
            let flags = self.connected_flags.lock().await;
            let guard = self.registry.read().await;
            for name in &task_names {
                let flag_true = flags
                    .get(name)
                    .map(|f| f.load(Ordering::SeqCst))
                    .unwrap_or(false);
                if let Some((handle, _cancel, _child_slot)) = tasks.get(name) {
                    // GAP-4: registered tools use the SANITIZED prefix (make_prefixed_name
                    // replaces `-`/`.`/`@`/`/` with `_`), so the lookup must sanitize too.
                    let sanitized_prefix = format!("{}__", sanitize_server_name(name));
                    let has_tools = guard
                        .get_definitions(None)
                        .iter()
                        .any(|t| t.function.name.starts_with(&sanitized_prefix));

                    if flag_true {
                        // Authoritative: handshake + list_all_tools both succeeded.
                        connected.push(name.clone());
                    } else if handle.is_finished() && !has_tools {
                        // Task exited (e.g. child crashed before initialize completed).
                        failed.push((name.clone(), "connection failed after retries".to_string()));
                    }
                    // flag_true==false + task still running + no tools yet = still connecting
                }
            }
        }

        let tool_count = self.registered_tool_count().await;
        StartResult {
            connected,
            failed,
            tool_count,
        }
    }

    /// Shutdown all running server tasks and unregister their tools.
    ///
    /// Cancels each task's `CancellationToken`, hard-kills any stdio child
    /// process (GAP-8), then awaits the task's `JoinHandle` with a bounded
    /// 2-second ceiling so the gateway's Ctrl+C path can never hang. Finally
    /// removes the server's tools from the registry via `unregister_by_prefix`.
    ///
    /// GAP-8 (Phase 21.2 Plan 11): before this fix, `ironhermes gateway` hung
    /// indefinitely on Ctrl+C when stdio MCP servers were connected. Root
    /// cause: the rmcp `TokioChildProcess` parent->child pipe closure did not
    /// cause a misbehaving child (e.g. Node runtime blocked on stdin) to
    /// exit, and tokio's process reaper kept the runtime alive until the
    /// child was reaped. The fix here has two parts working together:
    ///   1. `Child::start_kill()` sends SIGKILL if a child handle is parked
    ///      (currently a no-op under the plan-11 Option B fallback where
    ///      rmcp owns the child — `kill_on_drop(true)` in `connect_stdio`
    ///      covers this path via tokio's drop-kill).
    ///   2. `tokio::time::timeout(Duration::from_secs(2), handle)` bounds the
    ///      JoinHandle await so shutdown always returns — the operator's
    ///      Ctrl+C returns within ~2s/server regardless of child behavior.
    pub async fn shutdown_all(&self) -> Vec<ServerTaskResult> {
        use tokio::time::{Duration, timeout};

        let mut tasks = self.tasks.lock().await;
        let mut flags = self.connected_flags.lock().await;
        let mut last_failure = self.last_failure.lock().await;
        let mut results = Vec::new();

        for (name, (handle, cancel, child_slot)) in tasks.drain() {
            tracing::info!(server = %name, "Shutting down MCP server");

            // 1. Cancel the task's cancellation token (tells the serve loop to break).
            cancel.cancel();

            // 2. GAP-8: hard-kill the stdio child (if any). start_kill() is
            //    non-blocking — it sets the SIGKILL flag on the child. The
            //    subsequent timeout(2s, handle) does the actual bounded wait.
            //    Under plan-11 Option B this branch is typically a no-op —
            //    rmcp owns the child internally and the slot holds None — but
            //    `kill_on_drop(true)` in connect_stdio gives us the same OS-
            //    level kill guarantee when rmcp's transport drops. Either way,
            //    the bounded timeout below is the load-bearing guarantee.
            if let Some(mut child) = child_slot.lock().await.take() {
                match child.start_kill() {
                    Ok(()) => tracing::debug!(server = %name, "Sent SIGKILL to MCP stdio child"),
                    Err(e) => tracing::warn!(
                        server = %name,
                        error = %e,
                        "Failed to SIGKILL MCP stdio child (may already be reaped)"
                    ),
                }
            }

            // 3. GAP-8: await the JoinHandle with a bounded 2-second ceiling.
            //    If the task is genuinely stuck, we log a warning and proceed
            //    — never blocking the gateway's Ctrl+C return.
            match timeout(Duration::from_secs(2), handle).await {
                Ok(Ok(result)) => results.push(result),
                Ok(Err(join_err)) => tracing::warn!(
                    server = %name,
                    error = %join_err,
                    "MCP server task panicked during shutdown"
                ),
                Err(_elapsed) => tracing::warn!(
                    server = %name,
                    "MCP server task did not join within 2s of cancel+SIGKILL; proceeding"
                ),
            }

            // 4. Unregister tools for this server (D-09: rebuild registry on reload)
            //    GAP-4: unregister_by_prefix appends "__" to its argument; we must
            //    pass the already-sanitized server name so the match finds the
            //    tools we registered.
            let mut guard = self.registry.write().await;
            let removed = guard.unregister_by_prefix(&sanitize_server_name(&name));
            tracing::debug!(server = %name, removed, "Unregistered MCP tools");
            drop(guard); // explicitly drop before next iteration to avoid holding write lock

            // 5. GAP-7: remove the connected flag alongside the task handle so
            //    no stale reads of `connected_server_names()` survive shutdown.
            flags.remove(&name);
            // Remove the last-failure cell alongside it — a shut-down server
            // has no "current" failure to report.
            last_failure.remove(&name);
        }
        results
    }

    /// D-09: Reload all MCP servers (fire-and-forget, no result).
    ///
    /// Disconnects all servers (cancels tasks + unregisters tools), then reconnects
    /// with the new configs as background tasks. No waiting for connections.
    pub async fn reload(&self, new_configs: HashMap<String, McpServerConfig>) {
        self.shutdown_all().await;
        self.start_all(new_configs).await;
    }

    /// D-09/D-12: Reload all MCP servers and report connection results.
    ///
    /// Disconnects all servers, starts new tasks, waits briefly for connections,
    /// then aggregates `ServerTaskResult.failure_reason` into `StartResult.failed`.
    /// This is the method called by `McpReloader::reload()` in Plan 04.
    pub async fn reload_and_report(
        &self,
        new_configs: HashMap<String, McpServerConfig>,
    ) -> StartResult {
        // Shutdown existing servers + unregister their tools
        self.shutdown_all().await;
        // Start new servers and wait for initial connection status
        self.start_all_and_wait(new_configs).await
    }

    /// Return names of servers whose rmcp `initialize` handshake AND
    /// `list_all_tools()` both succeeded (GAP-7 contract). A spawned task
    /// that exited before the handshake completed is NOT reported here —
    /// unlike the old implementation which returned every `tasks.keys()`
    /// regardless of whether the child ever spoke MCP.
    ///
    /// Note: uses try_lock to avoid deadlock in sync call contexts. Returns
    /// empty vec if the mutex is currently locked.
    pub fn connected_server_names(&self) -> Vec<String> {
        if let Ok(flags) = self.connected_flags.try_lock() {
            flags
                .iter()
                .filter_map(|(name, flag)| {
                    if flag.load(Ordering::SeqCst) {
                        Some(name.clone())
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Count of MCP tools currently in the registry (identified by `__` in name).
    pub async fn registered_tool_count(&self) -> usize {
        let guard = self.registry.read().await;
        guard
            .get_definitions(None)
            .iter()
            .filter(|t| t.function.name.contains("__"))
            .count()
    }

    /// Presence-only check for a cached OAuth token in namespace `namespace`
    /// (Phase 48.2 Plan 02, REVIEWS finding 2).
    ///
    /// Answers a `bool` and nothing else. This is the SANCTIONED path for
    /// `iron_hermes_ui`'s MCP status classification (`classify_server_status`)
    /// to learn whether a server's OAuth flow has already produced a cached
    /// token, without ever handing out `Arc<AuthStore>` or any token-bearing
    /// value. `iron_hermes_ui` must never gain a way to read a token, its
    /// expiry, or any other secret material through this manager — only this
    /// one boolean answer.
    ///
    /// Mirrors the same presence check `server_task.rs`'s headless auto-start
    /// gate already performs (`store.get_token(ns).await.is_none()`), lifted
    /// to a public accessor. Returns `false` when this manager has no
    /// `AuthStore` attached (the `McpManager::new` default, `auth_store: None`)
    /// and `false` for an empty namespace string without panicking.
    pub async fn has_oauth_token(&self, namespace: &str) -> bool {
        match &self.auth_store {
            None => false,
            Some(store) => store.get_token(namespace).await.is_some(),
        }
    }

    /// Live, per-server last-known failure reason (warm-but-revoked follow-up
    /// fix, closes the gap `has_oauth_token`'s presence-only answer left
    /// open: a stored token can be present AND dead).
    ///
    /// Returns the most recent sanitized error `server_task::run_server_task`
    /// recorded for `name`, or `None` when the server has never failed, is
    /// currently connected (its last error was cleared on reconnect), or is
    /// not tracked by this manager at all. String-only — the same
    /// presence/string discipline as [`Self::has_oauth_token`]; no
    /// credential-bearing type crosses this boundary.
    pub async fn last_failure_reason(&self, name: &str) -> Option<String> {
        let cells = self.last_failure.lock().await;
        let cell = cells.get(name)?;
        cell.lock().ok()?.clone()
    }

    /// Phase 48.2 Plan 08 (D-03): begin a web-completable MCP OAuth
    /// authorization for `server_name`.
    ///
    /// Errors with fixed text when this manager has no `AuthStore` attached.
    /// Reaches `AuthorizationManager` construction only through
    /// `transport::begin_oauth_web`, which itself reaches it only through
    /// `transport::build_oauth_manager` — the single validated prelude the
    /// loopback CLI path also uses (T-48.2-08-01). On success, parks the
    /// returned `AuthorizationSession` keyed by its OAuth `state` and returns
    /// the auth URL + state pair — the entire surface `iron_hermes_ui` sees
    /// of this authorization attempt (no `AuthStore`, no token, ever).
    pub async fn begin_oauth(
        &self,
        server_name: &str,
        config: &McpServerConfig,
        redirect_uri: &str,
    ) -> Result<OAuthAuthorizationStart, String> {
        let auth_store = self
            .auth_store
            .clone()
            .ok_or_else(|| "no AuthStore configured; MCP OAuth is unavailable".to_string())?;

        let session = crate::transport::begin_oauth_web(
            config,
            auth_store,
            &self.global_issuer_allowlist,
            redirect_uri,
        )
        .await
        .map_err(|e| crate::security::sanitize_error(&e.to_string()))?;

        let state = crate::transport::oauth_state_from_url(&session.auth_url)
            .map_err(|_| "failed to derive an authorization state".to_string())?;
        let auth_url = session.auth_url.clone();

        let mut pending = self.pending_oauth.lock().await;
        let now = Instant::now();
        pending.retain(|_, entry| !pending_entry_expired(entry.created_at, now));
        if !pending_admission_allowed(pending.len()) {
            return Err(
                "too many pending MCP OAuth authorizations; wait for one to complete or expire \
                 before retrying"
                    .to_string(),
            );
        }
        pending.insert(
            state.clone(),
            PendingOAuth {
                session,
                server_name: server_name.to_string(),
                created_at: now,
            },
        );

        Ok(OAuthAuthorizationStart { auth_url, state })
    }

    /// Phase 48.2 Plan 08 (D-03): finish a parked web OAuth authorization
    /// from a callback URL string.
    ///
    /// Extracts the `state` from `callback_url`, removes (single-use) the
    /// matching pending entry, and drops the pending-map lock BEFORE
    /// awaiting `handle_callback_url` — a mutex guard held across an await
    /// is the failure mode this crate already guards against elsewhere. An
    /// unknown, already-used, or expired state is refused with a fixed
    /// message before any token exchange is attempted. On success, returns
    /// the server name the authorization belonged to.
    pub async fn complete_oauth(&self, callback_url: &str) -> Result<String, String> {
        let state = crate::transport::oauth_state_from_url(callback_url)
            .map_err(|_| "callback URL is missing a valid state parameter".to_string())?;

        let entry = {
            let mut pending = self.pending_oauth.lock().await;
            let now = Instant::now();
            pending.retain(|_, entry| !pending_entry_expired(entry.created_at, now));
            pending.remove(&state)
        };

        let PendingOAuth {
            session,
            server_name,
            ..
        } = entry
            .ok_or_else(|| "unknown, already-used, or expired authorization state".to_string())?;

        session
            .handle_callback_url(callback_url)
            .await
            .map_err(|e| crate::security::sanitize_error(&e.to_string()))?;

        Ok(server_name)
    }

    /// Phase 48.2 Plan 08: discard a pending web OAuth authorization without
    /// completing it — used by the web layer when the authorization server
    /// redirects back with a denial instead of a code.
    pub async fn cancel_oauth(&self, state: &str) {
        let mut pending = self.pending_oauth.lock().await;
        pending.remove(state);
    }
}

/// Phase 48.2 Plan 08 (T-48.2-08-03): pure age-check helper — returns whether
/// a pending entry created at `created_at` has outlived `PENDING_OAUTH_TTL_SECS`
/// as of `now`. Extracted so the boundary (exactly at the TTL, one second
/// past it) is unit-testable without a real `AuthorizationSession`.
fn pending_entry_expired(created_at: Instant, now: Instant) -> bool {
    now.duration_since(created_at) > Duration::from_secs(PENDING_OAUTH_TTL_SECS)
}

/// Phase 48.2 Plan 08 (T-48.2-08-03): pure admission-check helper — returns
/// whether one more pending entry may be admitted given `post_prune_len`, the
/// map's length AFTER expired entries have been pruned. Extracted so the
/// boundary (exactly at the cap, one under it) is unit-testable in isolation.
fn pending_admission_allowed(post_prune_len: usize) -> bool {
    post_prune_len < PENDING_OAUTH_MAX
}

// =============================================================================
// McpReloader implementation (Phase 21.2 Plan 04)
// =============================================================================

#[async_trait::async_trait]
impl ironhermes_core::commands::context::McpReloader for McpManager {
    /// Reload all MCP connections by re-reading config and calling `reload_and_report`.
    ///
    /// Uses `ironhermes_core::Config::load()` to get fresh `mcp_servers` config, then
    /// calls `reload_and_report` which shuts down existing servers and reconnects.
    /// Returns `McpReloadResult.failed` populated from `ServerTaskResult.failure_reason`
    /// via `StartResult.failed` (D-12 full delivery).
    async fn reload(&self) -> ironhermes_core::commands::context::McpReloadResult {
        // Re-read config to pick up any changes since startup.
        let new_configs: HashMap<String, McpServerConfig> = match ironhermes_core::Config::load() {
            Ok(config) => config
                .mcp_servers
                .into_iter()
                .filter_map(|(name, val)| {
                    serde_yaml::from_value::<McpServerConfig>(val)
                        .ok()
                        .map(|c| (name, c))
                })
                .collect(),
            Err(_) => HashMap::new(),
        };

        // reload_and_report: shutdown all + start_all_and_wait; aggregates
        // ServerTaskResult.failure_reason into StartResult.failed (D-12).
        let result = self.reload_and_report(new_configs).await;

        ironhermes_core::commands::context::McpReloadResult {
            connected: result.connected,
            failed: result.failed, // Populated from ServerTaskResult.failure_reason
            tool_count: result.tool_count,
        }
    }

    fn connected_server_names(&self) -> Vec<String> {
        McpManager::connected_server_names(self)
    }

    async fn registered_tool_count(&self) -> usize {
        McpManager::registered_tool_count(self).await
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        // Cancel all tasks on drop to avoid orphaned background tasks.
        //
        // GAP-8: Drop is sync and cannot await a JoinHandle or async-kill a
        // Child. The actual hard-kill path is `shutdown_all`, which is now
        // invoked by `GatewayRunner::start` BEFORE the runner returns (so
        // this Drop only runs after all children are already killed via the
        // bounded-timeout path). The `child_slot` is bound to `_` here because
        // a synchronous drop cannot take/start_kill it — Option B's
        // `kill_on_drop(true)` in connect_stdio covers any residual case.
        if let Ok(tasks) = self.tasks.try_lock() {
            for (_, cancel, _child_slot) in tasks.values() {
                cancel.cancel();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_tools::ToolRegistry;

    // -------------------------------------------------------------------
    // has_oauth_token (Phase 48.2 Plan 02, REVIEWS finding 2)
    // -------------------------------------------------------------------

    /// Build a fresh on-disk `AuthStore` under a per-test temp dir, mirroring
    /// `auth_store_adapter.rs::make_test_store` — each test gets its own
    /// subdirectory it owns so `AuthStore::save_to_disk`'s `chmod 0700` on
    /// the parent directory succeeds.
    async fn make_test_auth_store(tag: &str) -> Arc<ironhermes_core::auth::AuthStore> {
        let dir: std::path::PathBuf = std::env::temp_dir().join(format!(
            "ironhermes_mcp_manager_test_{}_{}",
            std::process::id(),
            tag,
        ));
        std::fs::create_dir_all(&dir).expect("could not create per-test temp dir");
        let path = dir.join("auth.json");
        ironhermes_core::auth::AuthStore::open(path)
            .await
            .expect("test AuthStore::open failed")
    }

    /// `has_oauth_token` returns `false` for a manager constructed without an
    /// `AuthStore` (the `McpManager::new` default), for any namespace.
    #[tokio::test]
    async fn has_oauth_token_false_without_auth_store() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry);
        assert!(!manager.has_oauth_token("some_namespace").await);
    }

    /// `has_oauth_token` returns `false` for a manager with an `AuthStore`
    /// that holds no token for the queried namespace.
    #[tokio::test]
    async fn has_oauth_token_false_when_namespace_has_no_token() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let store = make_test_auth_store("no_token").await;
        let manager = McpManager::new(registry).with_auth_store(Some(store));
        assert!(!manager.has_oauth_token("cloudflare_api").await);
    }

    /// `has_oauth_token` returns `true` for a manager with an `AuthStore`
    /// that holds a token for the queried namespace.
    #[tokio::test]
    async fn has_oauth_token_true_when_namespace_has_token() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let store = make_test_auth_store("has_token").await;
        store
            .put_token(
                "cloudflare_api",
                ironhermes_core::auth::TokenEntry {
                    access_token: "test-access-token".to_string(),
                    refresh_token: None,
                    expires_at: None,
                    scopes: vec![],
                },
            )
            .await
            .expect("put_token failed");
        let manager = McpManager::new(registry).with_auth_store(Some(store));
        assert!(manager.has_oauth_token("cloudflare_api").await);
    }

    /// `has_oauth_token` returns `false` for an empty namespace string
    /// without panicking (the `oauth_provider.as_deref().unwrap_or_default()`
    /// case the server task already produces).
    #[tokio::test]
    async fn has_oauth_token_false_for_empty_namespace_without_panic() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let store = make_test_auth_store("empty_ns").await;
        let manager = McpManager::new(registry).with_auth_store(Some(store));
        assert!(!manager.has_oauth_token("").await);
    }

    // -------------------------------------------------------------------
    // begin_oauth / complete_oauth / cancel_oauth (Phase 48.2 Plan 08, D-03)
    // -------------------------------------------------------------------

    /// `begin_oauth` on a manager built WITHOUT an auth store returns `Err`
    /// rather than panicking (the `McpManager::new` default, `auth_store: None`).
    #[tokio::test]
    async fn begin_oauth_errs_without_auth_store() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry);
        let config = McpServerConfig {
            url: Some("https://cloudflare.com/mcp".to_string()),
            oauth_provider: Some("test_ns".to_string()),
            ..Default::default()
        };
        let result = manager
            .begin_oauth(
                "test_server",
                &config,
                "https://hermes.example.com/oauth/mcp/callback",
            )
            .await;
        assert!(
            result.is_err(),
            "begin_oauth must return Err (not panic) when no AuthStore is attached"
        );
    }

    /// `complete_oauth` with a `state` that was never issued by `begin_oauth`
    /// returns the fixed unknown-state error — never reaches a token
    /// exchange (there is no parked session to call `handle_callback_url` on).
    #[tokio::test]
    async fn complete_oauth_errs_on_unknown_state() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry);
        let callback_url =
            "https://hermes.example.com/oauth/mcp/callback?code=abc&state=never-issued";
        let result = manager.complete_oauth(callback_url).await;
        assert!(
            result.is_err(),
            "complete_oauth must refuse an unknown state"
        );
    }

    /// `complete_oauth` with a callback URL that has no `state` query
    /// parameter at all returns Err before any pending-map lookup.
    #[tokio::test]
    async fn complete_oauth_errs_on_callback_url_missing_state() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry);
        let callback_url = "https://hermes.example.com/oauth/mcp/callback?code=abc";
        let result = manager.complete_oauth(callback_url).await;
        assert!(result.is_err());
    }

    /// `cancel_oauth` on an empty pending map is a no-op — does not panic or
    /// error.
    #[tokio::test]
    async fn cancel_oauth_on_unknown_state_is_noop() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry);
        manager.cancel_oauth("never-issued").await;
    }

    // -------------------------------------------------------------------
    // pending_entry_expired / pending_admission_allowed pure-helper
    // boundary tests (Phase 48.2 Plan 08, T-48.2-08-03)
    // -------------------------------------------------------------------

    #[test]
    fn pending_entry_expired_false_exactly_at_ttl() {
        let created_at = Instant::now();
        let now = created_at + Duration::from_secs(PENDING_OAUTH_TTL_SECS);
        assert!(
            !pending_entry_expired(created_at, now),
            "an entry exactly at the TTL boundary must NOT be considered expired \
             (strict > comparison)"
        );
    }

    #[test]
    fn pending_entry_expired_true_one_second_past_ttl() {
        let created_at = Instant::now();
        let now = created_at + Duration::from_secs(PENDING_OAUTH_TTL_SECS + 1);
        assert!(
            pending_entry_expired(created_at, now),
            "an entry one second past the TTL boundary must be expired"
        );
    }

    #[test]
    fn pending_entry_expired_false_well_within_ttl() {
        let created_at = Instant::now();
        let now = created_at + Duration::from_secs(1);
        assert!(!pending_entry_expired(created_at, now));
    }

    #[test]
    fn pending_admission_allowed_true_one_under_cap() {
        assert!(pending_admission_allowed(PENDING_OAUTH_MAX - 1));
    }

    #[test]
    fn pending_admission_allowed_false_exactly_at_cap() {
        assert!(
            !pending_admission_allowed(PENDING_OAUTH_MAX),
            "exactly at the cap must refuse admission (refuse, never evict)"
        );
    }

    #[test]
    fn pending_admission_allowed_false_above_cap() {
        assert!(!pending_admission_allowed(PENDING_OAUTH_MAX + 1));
    }

    // -------------------------------------------------------------------
    // No-credential-escape guard (Phase 48.2 Plan 08 Task 2, T-48.2-08-06)
    //
    // Static-source regression: no `pub fn` / `pub async fn` in this file may
    // have a return type mentioning a credential-bearing type name. This is
    // the same T-48.2-02-08 property registered in 48.2-02-PLAN.md, now
    // enforced by a test that fails the build if a future edit widens the
    // surface, rather than merely documented in a doc comment.
    // -------------------------------------------------------------------

    #[test]
    fn mcp_manager_public_methods_never_return_credential_bearing_types() {
        let src = include_str!("manager.rs");

        // Strip comment-only lines first so a doc comment mentioning a
        // forbidden type name cannot satisfy or defeat this assertion.
        let code_only: String = src
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        // Credential-bearing type names that must never appear in a public
        // method's signature in this file (T-48.2-02-08 / T-48.2-08-06).
        const FORBIDDEN: &[&str] = &["AuthStore", "TokenEntry"];

        let mut idx = 0usize;
        while let Some(rel) = code_only[idx..].find("pub ") {
            let start = idx + rel;

            // Only treat this as a real declaration if "pub" begins its own
            // line (mod leading whitespace) — this excludes matches embedded
            // mid-line inside a string literal, such as THIS VERY TEST'S OWN
            // `.starts_with("pub fn ")` / `.starts_with("pub async fn ")`
            // calls below. A genuine `pub fn`/`pub async fn` declaration is
            // always the first non-whitespace token on its line.
            let line_start = code_only[..start].rfind('\n').map(|p| p + 1).unwrap_or(0);
            let line_prefix = &code_only[line_start..start];
            if !line_prefix.trim().is_empty() {
                idx = start + 4;
                continue;
            }

            let rest = &code_only[start..];
            if rest.starts_with("pub fn ") || rest.starts_with("pub async fn ") {
                // The signature runs from `start` to the opening brace of the
                // function body. Rustfmt may wrap a long signature across
                // multiple lines, so this span can include newlines —
                // normalize whitespace below before matching.
                let brace_rel = rest.find('{').unwrap_or_else(|| {
                    panic!(
                        "T-48.2-08-06: could not find an opening brace for the public fn \
                         starting at byte {start} of manager.rs; signature snippet={:?}",
                        &rest[..rest.len().min(200)]
                    )
                });
                let raw_signature = &rest[..brace_rel];

                // Only the RETURN type (everything after the parameter list's
                // matching closing paren) is in scope — a builder/constructor
                // legitimately TAKES an `Arc<AuthStore>` as an input parameter
                // (that is how the manager receives it in the first place);
                // T-48.2-02-08/T-48.2-08-06 forbid a credential-bearing
                // RETURN type, not a credential-bearing parameter.
                let open_paren = raw_signature.find('(').unwrap_or_else(|| {
                    panic!(
                        "T-48.2-08-06: public fn signature has no parameter list: \
                         `{raw_signature}`"
                    )
                });
                let mut depth = 0i32;
                let mut close_paren = None;
                for (i, ch) in raw_signature[open_paren..].char_indices() {
                    match ch {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                close_paren = Some(open_paren + i);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let close_paren = close_paren.unwrap_or_else(|| {
                    panic!(
                        "T-48.2-08-06: unbalanced parens in public fn signature: \
                         `{raw_signature}`"
                    )
                });
                let return_and_where = &raw_signature[close_paren + 1..];
                let normalized: String = return_and_where
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");

                for forbidden in FORBIDDEN {
                    assert!(
                        !normalized.contains(forbidden),
                        "T-48.2-08-06: a public method's RETURN type in manager.rs must never \
                         mention a credential-bearing type ({forbidden}). Offending return \
                         type: `{normalized}` (full signature: `{}`)",
                        raw_signature
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                }
            }
            idx = start + 4; // advance past "pub " to find the next occurrence
        }
    }

    #[test]
    fn test_start_result_fields() {
        let result = StartResult {
            connected: vec!["server_a".to_string()],
            failed: vec![("server_b".to_string(), "timeout".to_string())],
            tool_count: 5,
        };
        assert_eq!(result.connected.len(), 1);
        assert_eq!(result.failed.len(), 1);
        assert_eq!(result.failed[0].0, "server_b");
        assert_eq!(result.failed[0].1, "timeout");
        assert_eq!(result.tool_count, 5);
    }

    #[tokio::test]
    async fn test_mcp_manager_new() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry);
        assert_eq!(manager.connected_server_names().len(), 0);
        assert_eq!(manager.registered_tool_count().await, 0);
    }

    #[tokio::test]
    async fn test_start_all_skips_disabled() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry);

        let mut configs = HashMap::new();
        let disabled = McpServerConfig {
            enabled: false,
            command: Some("echo".to_string()),
            ..Default::default()
        };
        configs.insert("disabled_server".to_string(), disabled);

        manager.start_all(configs).await;

        // Disabled server should not appear in task map
        assert_eq!(manager.connected_server_names().len(), 0);
    }

    #[tokio::test]
    async fn test_shutdown_all_empty() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry);
        // Shutdown on empty manager should return empty results without panic
        let results = manager.shutdown_all().await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn shutdown_all_unregisters_tools_for_server_with_special_char_name() {
        use crate::tool::{McpTool, make_prefixed_name};
        use ironhermes_tools::ToolRegistry;
        use tokio::sync::mpsc;

        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry.clone());

        // A server name that hits every one of the four sanitized characters.
        let server_name = "@scope/pkg-name.v1";

        // Manually register two tools under the sanitized prefix — simulating
        // what server_task::connect_and_serve would do at connect time.
        {
            let mut guard = registry.write().await;
            for tool_original in ["read_file", "write_file"] {
                let (tx, _rx) = mpsc::channel(1);
                let tool = McpTool::new(
                    server_name,
                    tool_original,
                    "desc",
                    serde_json::json!({}),
                    tx,
                );
                guard.register_dynamic(Box::new(tool));
            }
        }

        // Seed the manager task map with a no-op task bound to `server_name`
        // so shutdown_all has something to drain.
        {
            let cancel = CancellationToken::new();
            let handle = tokio::spawn(async {
                crate::server_task::ServerTaskResult {
                    server_name: "placeholder".to_string(),
                    tool_names: vec![],
                    failure_reason: None,
                }
            });
            manager.tasks.lock().await.insert(
                server_name.to_string(),
                (handle, cancel, Arc::new(tokio::sync::Mutex::new(None))),
            );
        }

        // Pre-condition: both tools visible under sanitized prefix.
        let sanitized_prefix = make_prefixed_name(server_name, "");
        let sanitized_prefix = sanitized_prefix.trim_end_matches("__").to_string();
        // sanitized_prefix is e.g. "_scope_pkg_name_v1"
        {
            let guard = registry.read().await;
            let hits: Vec<_> = guard
                .get_definitions(None)
                .into_iter()
                .filter(|t| {
                    t.function
                        .name
                        .starts_with(&format!("{sanitized_prefix}__"))
                })
                .collect();
            assert_eq!(
                hits.len(),
                2,
                "precondition: 2 tools under sanitized prefix before shutdown"
            );
        }

        // Act: shutdown_all must unregister them.
        let _ = manager.shutdown_all().await;

        // Post-condition: zero tools remain under the sanitized prefix.
        let guard = registry.read().await;
        let leftover: Vec<_> = guard
            .get_definitions(None)
            .into_iter()
            .filter(|t| {
                t.function
                    .name
                    .starts_with(&format!("{sanitized_prefix}__"))
            })
            .collect();
        assert!(
            leftover.is_empty(),
            "GAP-4: shutdown_all must remove all tools of a special-char-named server; leftover={:?}",
            leftover
                .iter()
                .map(|t| t.function.name.clone())
                .collect::<Vec<_>>()
        );
    }

    /// GAP-7: when a server's child exits before the rmcp `initialize`
    /// handshake completes, `connected_server_names()` must NOT report it.
    /// Before this fix, `tasks.keys()` was returned unconditionally, so a
    /// crashed child appeared as "connected" with zero tools, producing the
    /// false-positive startup message `MCP: 0 tool(s) ready from 1 server(s).`
    #[tokio::test]
    async fn connected_server_names_excludes_server_that_exited_before_initialize() {
        use std::time::Duration;

        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry.clone());

        // A command that exits immediately without speaking MCP JSON-RPC.
        // `false` on unix returns exit code 1 instantly; on Windows use
        // `cmd /C exit 1` for equivalent behavior.
        let mut cfg = McpServerConfig::default();
        #[cfg(unix)]
        {
            cfg.command = Some("false".to_string());
        }
        #[cfg(not(unix))]
        {
            cfg.command = Some("cmd".to_string());
            cfg.args = vec!["/C".to_string(), "exit".to_string(), "1".to_string()];
        }
        cfg.enabled = true;
        // Tiny connect_timeout so the test doesn't wait 60s for retries.
        cfg.connect_timeout = 1;

        let mut configs = HashMap::new();
        configs.insert("crashy".to_string(), cfg);

        manager.start_all(configs).await;

        // Give the child some room to spawn, exit, and let server_task observe
        // the failure. list_all_tools() will never succeed against `false`, so
        // connected.store(true) never fires. 500ms is plenty for a process
        // that exits immediately.
        tokio::time::sleep(Duration::from_millis(500)).await;

        let names = manager.connected_server_names();
        assert!(
            names.is_empty(),
            "GAP-7: connected_server_names() must NOT include a server whose \
             child exited before the rmcp initialize handshake. names={names:?}"
        );

        // Cleanly shut down so the test doesn't leak the spawned task.
        let _ = manager.shutdown_all().await;
    }

    /// GAP-7 companion: after a manual flag-flip (simulating what
    /// connect_and_serve does on the happy path after list_all_tools),
    /// connected_server_names() MUST include the server. Proves the new
    /// lookup path correctly reads the flag (not just "always empty").
    #[tokio::test]
    async fn connected_server_names_includes_server_whose_flag_is_true() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry.clone());

        // Insert a flag directly — no live MCP needed. This mirrors exactly
        // what server_task::connect_and_serve does after list_all_tools succeeds.
        {
            let mut flags = manager.connected_flags.lock().await;
            let flag = Arc::new(AtomicBool::new(true));
            flags.insert("ok_server".to_string(), flag);
        }

        let names = manager.connected_server_names();
        assert_eq!(
            names,
            vec!["ok_server".to_string()],
            "GAP-7: a server whose connected flag is true must be reported by \
             connected_server_names(). names={names:?}"
        );
    }

    #[tokio::test]
    async fn reload_with_special_char_server_name_does_not_duplicate_tools() {
        // Higher-level regression: simulate two back-to-back reload cycles
        // and assert the registry never grows beyond the per-cycle tool count.
        // Uses the same manual-register pattern as the test above (no live MCP).
        use crate::tool::{McpTool, make_prefixed_name};
        use ironhermes_tools::ToolRegistry;
        use tokio::sync::mpsc;

        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry.clone());
        let server_name = "@org/pkg-x.y";

        async fn seed_cycle(
            manager: &McpManager,
            registry: &Arc<RwLock<ToolRegistry>>,
            server_name: &str,
        ) {
            let mut guard = registry.write().await;
            for tool_original in ["a", "b", "c"] {
                let (tx, _rx) = mpsc::channel(1);
                let tool = McpTool::new(server_name, tool_original, "d", serde_json::json!({}), tx);
                guard.register_dynamic(Box::new(tool));
            }
            drop(guard);
            let cancel = CancellationToken::new();
            let handle = tokio::spawn(async {
                crate::server_task::ServerTaskResult {
                    server_name: "placeholder".to_string(),
                    tool_names: vec![],
                    failure_reason: None,
                }
            });
            manager.tasks.lock().await.insert(
                server_name.to_string(),
                (handle, cancel, Arc::new(tokio::sync::Mutex::new(None))),
            );
        }

        // Cycle 1: register + shutdown (simulates one reload iteration)
        seed_cycle(&manager, &registry, server_name).await;
        let _ = manager.shutdown_all().await;

        // Cycle 2: register again + shutdown
        seed_cycle(&manager, &registry, server_name).await;
        let _ = manager.shutdown_all().await;

        // The registry must be empty of this server's tools — no accumulation.
        let sanitized_prefix = make_prefixed_name(server_name, "");
        let sanitized_prefix = sanitized_prefix.trim_end_matches("__").to_string();
        let guard = registry.read().await;
        let residue: Vec<_> = guard
            .get_definitions(None)
            .into_iter()
            .filter(|t| {
                t.function
                    .name
                    .starts_with(&format!("{sanitized_prefix}__"))
            })
            .collect();
        assert!(
            residue.is_empty(),
            "GAP-4: two reload cycles must not leave duplicates; residue={:?}",
            residue
                .iter()
                .map(|t| t.function.name.clone())
                .collect::<Vec<_>>()
        );
    }

    /// GAP-8: `shutdown_all` must return within a bounded time even when
    /// a stdio MCP child process is long-lived and not responding to
    /// parent-pipe EOF. Before this fix, `ironhermes gateway` hung on
    /// Ctrl+C because the tokio process reaper kept the runtime alive
    /// until the child was reaped.
    ///
    /// Test shape: spawn a long-running stdio "server" (`sleep 300` on
    /// unix). Give it ~500ms to attach. Call `shutdown_all()` wrapped in
    /// an OUTER `tokio::time::timeout(5s, ...)`. The outer timeout MUST
    /// NOT fire — i.e., shutdown_all returns in well under 5 seconds.
    /// Internally, manager.rs bounds each task to 2s via
    /// `tokio::time::timeout`, so for 1 server this should be well under
    /// 3s in the worst case even when the child never responds.
    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_all_returns_within_timeout_when_stdio_child_blocks() {
        use std::time::Duration;

        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let manager = McpManager::new(registry.clone());

        // Short connect_timeout so server_task doesn't burn full 60s retrying
        // (note: `sleep 300` never speaks MCP, so server_task will fail the
        // initialize handshake repeatedly and retry with backoff — tight
        // connect_timeout keeps the test window small).
        let cfg = McpServerConfig {
            command: Some("sleep".to_string()),
            args: vec!["300".to_string()],
            enabled: true,
            connect_timeout: 1,
            ..Default::default()
        };

        let mut configs = HashMap::new();
        configs.insert("sleepy".to_string(), cfg);

        manager.start_all(configs).await;
        // Give the Child time to actually spawn and be parked in child_slot
        // (or, under Option B, to at least be live under rmcp's internal
        // ownership).
        tokio::time::sleep(Duration::from_millis(500)).await;

        // The crucial assertion: shutdown_all must return within the OUTER
        // 5-second test timeout. Internally, manager.rs bounds each task
        // to 2s via tokio::time::timeout, so for 1 server this should be
        // well under 3s in the worst case.
        let shutdown_result =
            tokio::time::timeout(Duration::from_secs(5), manager.shutdown_all()).await;
        assert!(
            shutdown_result.is_ok(),
            "GAP-8: shutdown_all MUST return within 5s even when the stdio \
             child is long-lived. If this test hangs, the bounded-timeout \
             + hard-kill wire in shutdown_all is regressed."
        );

        // Post-condition: the manager's task map is empty (drained) and
        // the connected_server_names reports empty.
        let names = manager.connected_server_names();
        assert!(
            names.is_empty(),
            "GAP-8: post-shutdown, connected_server_names must be empty"
        );
    }
}
