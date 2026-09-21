//! Phase 25.1 — Shared lazy-spawned chromiumoxide session for all 11 browser_* tools.
//! Phase 53 (ADR-0005) — `spawn` now branches on `browser.backend`: Chromium
//! (default, unchanged) or Obscura, locally spawned via `obscura serve` on an
//! OS-assigned ephemeral port. `connect(ws_url)` (Plan 02 Task 3) attaches to a
//! remote CDP endpoint this process does not own.
//!
//! Held behind `Arc<tokio::sync::Mutex<Option<BrowserSession>>>` on AgentLoop (plan 09)
//! so all 11 browser_* tools share one chromium process. First browser_* call spawns;
//! browser_close drops back to None; AgentLoop drop aborts the CDP pump task (via
//! `JoinHandle`'s own drop semantics — that only stops the *task*, not any spawned
//! process) and, per Phase 53 Plan 03's `impl Drop for BrowserSession`, best-effort
//! `start_kill()`s a locally-spawned Obscura child so it cannot outlive the session.

use std::collections::HashMap;
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use chromiumoxide::Handler;
use chromiumoxide::browser::{Browser, BrowserConfig as CdpBrowserConfig};
use chromiumoxide::page::Page;
use futures::StreamExt;
use tokio::process::{Child, Command};
use tracing::{debug, warn};

use ironhermes_core::config::{BrowserBackend, BrowserConfig};

use crate::registry::Prerequisite;

/// Phase 25.1 D-03: lazy-spawned CDP session shared across all 11 browser_* tools.
///
/// Lifecycle:
///   * `spawn(config)` — called on first browser_* tool use; branches on
///     `browser.backend` (Phase 53) to launch Chromium or spawn a local
///     `obscura serve` process
///   * `connect(ws_url)` (Phase 53, Plan 02 Task 3) — attaches to a remote CDP
///     endpoint nobody in this process started; owns no child process
///   * Reused across subsequent browser_* calls
///   * `close()` (Phase 53 Plan 03) — explicit teardown by browser_close tool,
///     dispatched by ownership rather than one unconditional sequence: a
///     locally-spawned child (`child: Some(_)`, i.e. Obscura local-spawn) is
///     SIGTERM'd, given a bounded grace period, then hard-killed via
///     `Child::start_kill()` and reaped with `wait()`; Chromium (always
///     `child: None`, owned by chromiumoxide/the OS) keeps sending CDP
///     `Browser.close` exactly as before; a remote Obscura endpoint
///     (`connect()`, `child: None`) only drops the connection and must never
///     send `Browser.close` to a server this process does not own
///   * Implicit teardown when the `Arc<Mutex<Option<Self>>>` is dropped
///     (AgentLoop drop, or any other implicit drop) — `impl Drop for
///     BrowserSession` best-effort `start_kill()`s any locally-spawned child
///     so it cannot outlive the session. `Drop` cannot `await`, so this is
///     deliberately best-effort and does not reap; `close()` remains the path
///     that gracefully terminates and reaps.
pub struct BrowserSession {
    /// chromiumoxide Browser handle.
    pub browser: Browser,
    /// The single active Page (single-page model in v2.1 — D-03 / OUT-OF-SCOPE).
    pub page: Page,
    /// Phase 25.1 D-10: ref table populated by browser_snapshot.
    /// Maps sequential u64 IDs (1, 2, ...) → opaque element selector string
    /// (data-ironhermes-ref attribute injected at snapshot time per RESEARCH OQ-1 fix).
    /// CLEARED at the start of each browser_snapshot call.
    pub ref_table: HashMap<u64, String>,
    /// Phase 25.1 D-08: console log buffer drained by browser_console mode:"log".
    /// Cleared on browser_close + on each browser_navigate.
    pub console_buffer: Vec<serde_json::Value>,
    /// CDP websocket pump handle. Aborted on Drop or close().
    handler_task: tokio::task::JoinHandle<()>,
    /// Phase 53 (D-03/D-05): the locally-spawned Obscura child process, when this
    /// session owns one. `Some` means this session spawned `obscura serve` and is
    /// responsible for reaping it — the single field `close()` and `Drop` read to
    /// decide what they may kill (Plan 03). `None` means either the session drives
    /// Chromium (whose lifecycle chromiumoxide/the OS owns via `browser.close()`)
    /// or the session was built by `connect()` against a remote endpoint this
    /// process did not start and must never terminate.
    child: Option<Child>,
    /// Phase 53 (D-01): which CDP engine this session is driving. Set once at
    /// construction and never mutated afterward. Consulted by `close()` (Plan 03)
    /// when `child` is `None` to choose CDP `Browser.close` (Chromium) vs
    /// drop-only (Obscura remote).
    backend: BrowserBackend,
}

/// Phase 53 Plan 03 (Task 2, T-53-03-02): typed session-start failures.
/// Constructed by `spawn`/`connect`'s Obscura branches and by
/// `probe_render_support`, wrapped through `anyhow` via the blanket
/// `From<E: std::error::Error>` impl so `spawn`/`connect`'s existing
/// `anyhow::Result` signatures are unchanged. Task 3's `diagnose` downcasts
/// the returned `anyhow::Error` back to this type (`downcast_ref`) to
/// classify the outcome without a second copy of the probe or resolver
/// logic.
#[derive(Debug, thiserror::Error)]
pub enum BrowserStartError {
    /// Neither the configured path, the relevant env var, nor PATH resolved
    /// a binary for the named engine.
    #[error("{binary} binary not found. {hint}")]
    BinaryNotFound { binary: &'static str, hint: String },
    /// `tokio::process::Command::spawn` itself failed (not found, exec
    /// permission, etc.) — distinct from the readiness poll timing out.
    #[error("failed to spawn `{command}`: {source}")]
    SpawnFailed {
        command: String,
        #[source]
        source: std::io::Error,
    },
    /// The spawned process's HTTP control plane never answered within the
    /// configured timeout.
    #[error("{what} did not become ready within {timeout_secs}s")]
    ReadinessTimeout { what: String, timeout_secs: u64 },
    /// The CDP websocket handshake itself failed, local or remote.
    #[error("{context}: {message}")]
    ConnectFailed { context: String, message: String },
    /// D-05: both render-probe legs did not agree after a confirmed
    /// navigation — the binary appears to have been built without
    /// `--features render` (release archives publish this as the
    /// `-no-render` suffix), so layout is absent and every `browser_*` tool
    /// would return plausible but wrong answers against it.
    #[error(
        "Obscura build has no layout support (screenshot_ok={screenshot_ok}, \
         hidden_reports_none={hidden_reports_none}). Rebuild with `--features render` or use a \
         release archive without the `-no-render` suffix."
    )]
    RenderUnsupported {
        screenshot_ok: bool,
        hidden_reports_none: bool,
    },
}

impl BrowserSession {
    /// Phase 25.1 D-03: lazy-spawn a session via chromiumoxide.
    /// Phase 53 (ADR-0005 D-01): branches on `config.backend` BEFORE any Chromium
    /// work. `Obscura` resolves the Obscura binary, self-binds an ephemeral port
    /// (D-03), spawns `obscura serve --port <N>`, polls readiness, and connects
    /// over CDP. Anything else falls through to the pre-53 Chromium path below,
    /// textually unchanged.
    ///
    /// Returns Err if the configured backend's binary is not discoverable, or if
    /// the Obscura process fails to spawn/become ready/connect. The returned
    /// session has a fresh `about:blank` Page; the caller (typically
    /// browser_navigate) navigates next.
    pub async fn spawn(config: &BrowserConfig) -> anyhow::Result<Self> {
        if config.backend == BrowserBackend::Obscura {
            // D-01: cdp_url is consulted ONLY here (inside the Obscura arm). Some(url)
            // selects remote-connect and spawns nothing; None selects local-spawn below.
            if let Some(url) = config.cdp_url.as_deref() {
                return Self::connect(url).await;
            }

            let binary = find_obscura_binary(config.obscura_path.as_deref()).ok_or_else(|| {
                BrowserStartError::BinaryNotFound {
                    binary: "Obscura",
                    hint: "Set OBSCURA_PATH or browser.obscura_path, or install obscura. \
                           Searched: OBSCURA_PATH, browser.obscura_path, PATH"
                        .to_string(),
                }
            })?;

            debug!(binary = %binary.display(), "Phase 53: spawning obscura serve");

            let (mut child, port) = spawn_obscura_serve(config, &binary).await?;

            if let Err(e) =
                wait_for_obscura_ready(port, Duration::from_secs(config.timeout_seconds)).await
            {
                let _ = child.start_kill();
                return Err(e);
            }

            let ws_url = format!("ws://127.0.0.1:{port}");
            let (browser, handler) = match Browser::connect(ws_url.clone()).await {
                Ok(pair) => pair,
                Err(e) => {
                    let _ = child.start_kill();
                    return Err(BrowserStartError::ConnectFailed {
                        context: format!("Obscura CDP connect to '{ws_url}' failed"),
                        message: e.to_string(),
                    }
                    .into());
                }
            };

            let session =
                build_session(browser, handler, Some(child), BrowserBackend::Obscura).await?;

            // D-05: refuse a non-render build here, before handing the
            // session to any browser_* tool. Terminate the locally-spawned
            // child before returning — reusing close()'s own per-mode
            // teardown (T-53-03-01) rather than a second, parallel cleanup
            // path.
            if let Err(e) = probe_render_support(&session.page).await {
                let _ = session.close().await;
                return Err(e.into());
            }

            return Ok(session);
        }

        // D-02: cdp_url is inert under backend: chromium — warn once at session
        // start, never swap engines or spawn anything from it. Named fields only;
        // browser.cdp_url may carry credentials, so log the field name, not the
        // value.
        if config.cdp_url.is_some() {
            tracing::warn!(
                backend = "chromium",
                inert_setting = "browser.cdp_url",
                honor_via = "browser.backend",
                "browser.cdp_url is set but ignored because browser.backend is chromium; \
                 set browser.backend: obscura to use it"
            );
        }

        let binary = find_chromium_binary(config.chromium_path.as_deref())
            .ok_or_else(|| anyhow::anyhow!(
                "Chromium/Chrome binary not found. Set BROWSER_PATH or browser.chromium_path, \
                 or install chromium. Searched: BROWSER_PATH, CHROMIUM_PATH, PATH, /Applications, /usr/bin, %PROGRAMFILES%"
            ))?;

        debug!(binary = %binary.display(), headed = config.headed, no_sandbox = config.no_sandbox,
               "Phase 25.1: spawning chromium");

        let mut builder = CdpBrowserConfig::builder()
            .chrome_executable(binary)
            .launch_timeout(Duration::from_secs(config.timeout_seconds));

        // Phase 26.3: resolve persistent profile directory.
        // Call get_hermes_home() at call time so Phase 24 profile pivot works correctly.
        // Empty-string check is defensive — operator-set "" should fall through to default.
        let user_data_dir: std::path::PathBuf = match config.user_data_dir.as_deref() {
            Some(p) if !p.is_empty() => {
                let resolved = std::path::PathBuf::from(p);
                if resolved.is_relative() {
                    anyhow::bail!("browser.user_data_dir '{}' must be an absolute path", p);
                }
                resolved
            }
            _ => ironhermes_core::get_hermes_home().join("browser-profile"),
        };
        // Phase 26.3.2: SingletonLock resilience — check before binding to persistent profile.
        match ironhermes_core::browser_profile::reconcile_singleton_lock(&user_data_dir) {
            ironhermes_core::browser_profile::SingletonOutcome::UseProfile => {
                builder = builder.user_data_dir(&user_data_dir);
            }
            ironhermes_core::browser_profile::SingletonOutcome::UseEphemeral => {
                // Lock held by a live process — do NOT set user_data_dir.
                // chromiumoxide defaults to its own temp dir (pre-26.3 ephemeral behavior).
                // warn! was already emitted inside reconcile_singleton_lock (D-05).
            }
        }
        if let Err(e) = std::fs::create_dir_all(&user_data_dir) {
            tracing::warn!(
                path = %user_data_dir.display(),
                error = %e,
                "Phase 26.3: failed to pre-create user_data_dir; chromium launch may fail"
            );
        }

        if config.headed {
            // chromiumoxide 0.9 default IS headless; .with_head() opts INTO headed.
            builder = builder.with_head();
        }
        if config.no_sandbox {
            builder = builder.no_sandbox();
        }

        let cdp_cfg = builder
            .arg("--disable-gpu")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .build()
            .map_err(|e| anyhow::anyhow!("BrowserConfig build failed: {e}"))?;

        let (browser, handler) = Browser::launch(cdp_cfg)
            .await
            .map_err(|e| anyhow::anyhow!("chromium launch failed: {e}"))?;

        build_session(browser, handler, None, BrowserBackend::Chromium).await
    }

    /// Phase 53 (ADR-0005, Plan 02 Task 3): attach to a remote CDP endpoint nobody
    /// in this process started. Performs no binary discovery, no Chromium profile
    /// work, and spawns nothing — `connect` is the whole of it. The returned
    /// session's `child` is `None`, so Plan 03's per-mode teardown must never send
    /// it `Browser.close` (D-01): drop the connection only.
    ///
    /// Phase 53 Plan 03 (Task 2, D-05): also runs the render probe before
    /// returning — this covers `spawn`'s remote `cdp_url` arm too, since it
    /// delegates to `connect`.
    pub async fn connect(ws_url: &str) -> anyhow::Result<Self> {
        let (browser, handler) =
            Browser::connect(ws_url)
                .await
                .map_err(|e| BrowserStartError::ConnectFailed {
                    context: format!("Obscura CDP connect to '{ws_url}' (browser.cdp_url) failed"),
                    message: e.to_string(),
                })?;
        let session = build_session(browser, handler, None, BrowserBackend::Obscura).await?;

        if let Err(e) = probe_render_support(&session.page).await {
            let _ = session.close().await;
            return Err(e.into());
        }

        Ok(session)
    }

    /// Phase 53 Plan 05: which CDP engine this session is actually driving.
    /// `browser_vision` (and any future per-backend tool logic) must branch
    /// on the session that actually ran, not on config re-read from disk —
    /// config can change under a live session — so this is the accessor to
    /// use instead of `Config::load()`.
    pub fn backend(&self) -> BrowserBackend {
        self.backend.clone()
    }

    /// Phase 25.1 D-03 / browser_close (plan 04): explicit teardown.
    /// After this returns, the BrowserSession should be dropped (Option set to None
    /// in the Arc<Mutex<Option<...>>>).
    ///
    /// Phase 53 Plan 03 (T-53-03-01/03): dispatches on ownership rather than
    /// running one unconditional sequence. A locally-spawned child (`child:
    /// Some(_)`, i.e. Obscura local-spawn) is terminated directly — SIGTERM,
    /// a bounded grace period, then a hard kill via `Child::start_kill()`,
    /// reaped with `wait()` — and CDP `Browser.close` is never sent to it:
    /// the CDP endpoint goes away with the process itself. Chromium (always
    /// `child: None`) keeps sending `Browser.close` exactly as before. A
    /// remote Obscura endpoint built by `connect()` (`child: None`, `backend:
    /// Obscura`) drops the connection only — sending `Browser.close` there
    /// would be a cross-tenant kill issued against a server this process
    /// does not own; Obscura's current handler happens to be a no-op, but
    /// that is an implementation detail it may change, not a protocol
    /// guarantee (`cdp_url` is a generic CDP-endpoint field).
    ///
    /// `handler_task.abort()` runs on every branch.
    pub async fn close(mut self) -> anyhow::Result<()> {
        match teardown_action(self.child.is_some(), self.backend.clone()) {
            TeardownAction::TerminateChild => {
                if let Some(mut child) = self.child.take() {
                    // SIGTERM first, so a locally-spawned obscura serve
                    // process gets a chance to unwind before any hard kill
                    // (T-53-03-03). Never send Browser.close here — the CDP
                    // endpoint goes away with the process itself.
                    #[cfg(unix)]
                    if let Some(pid) = child.id() {
                        use nix::sys::signal::{self, kill};
                        use nix::unistd::Pid;
                        let _ = kill(Pid::from_raw(pid as i32), signal::Signal::SIGTERM);
                    }
                    // tokio::process exposes no SIGTERM equivalent off Unix —
                    // Child::start_kill() (SIGKILL-equivalent) is the only
                    // termination primitive available there, so non-Unix
                    // goes straight to the hard kill below rather than
                    // silently downgrading the "graceful" path to a no-op on
                    // every non-Unix platform.
                    #[cfg(not(unix))]
                    let _ = child.start_kill();

                    // Bounded grace period before escalating — never an
                    // unbounded wait (T-53-03-03/04).
                    if tokio::time::timeout(CHILD_TERM_GRACE, child.wait())
                        .await
                        .is_err()
                    {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                    }
                }
            }
            TeardownAction::SendBrowserClose => {
                // browser.close() sends CDP Browser.close so chromium's own
                // process exits on its own — the pre-53 behavior, unchanged
                // for this branch.
                let _ = self.browser.close().await;
            }
            TeardownAction::DropOnly => {
                // A remote endpoint this process did not start: dropping
                // `self.browser` below closes the websocket, which is the
                // whole of the teardown this process is entitled to
                // perform. Send nothing over CDP.
            }
        }
        self.handler_task.abort();
        Ok(())
    }

    /// Phase 25.1 D-15: validate URL host against the allowlist.
    /// Empty list = allow all (D-15). Non-empty = exact-match required.
    /// Returns Ok(()) when allowed; Err with the allowed list when blocked.
    pub fn validate_navigation_url(allowed_domains: &[String], url: &str) -> anyhow::Result<()> {
        if allowed_domains.is_empty() {
            return Ok(());
        }
        let host = extract_host(url)
            .ok_or_else(|| anyhow::anyhow!("invalid URL '{}': cannot extract host", url))?;
        if allowed_domains.iter().any(|d| d == &host) {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "{}",
                serde_json::json!({
                    "error": "domain_blocked",
                    "url": url,
                    "host": host,
                    "allowed": allowed_domains,
                    "hint": "Add the host to browser.allowed_domains or leave the list empty to allow all"
                })
            ))
        }
    }
}

/// Hand-rolled host extractor — avoids adding the `url` crate dep (OQ-4 resolution).
/// Handles `scheme://host/path`, `scheme://host:port/path`, etc.
fn extract_host(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://")?.1;
    let host_with_port = after_scheme.split('/').next()?;
    Some(host_with_port.split(':').next()?.to_string())
}

/// Phase 53 Plan 03 (T-53-03-03): bounded grace period between SIGTERM and the
/// hard-kill escalation for a locally-spawned child in `close()`. Never
/// unbounded — `close()` always escalates to `Child::start_kill()` once this
/// elapses. Kanban's own worker-teardown sequence
/// (`crates/ironhermes-kanban/src/dispatcher.rs:917-927`) uses a 5s grace for
/// a long-running task process; `obscura serve` is a lighter-weight local
/// process, so 3s is enough headroom to unwind cleanly without keeping
/// `browser_close` blocked for long.
const CHILD_TERM_GRACE: Duration = Duration::from_secs(3);

/// Phase 53 Plan 03 (T-53-03-01): the three-way teardown decision `close()`
/// acts on, factored out so it is unit-testable without a live CDP
/// connection. Ownership (whether `child` is `Some`) always wins over
/// `backend` — a locally-spawned process is terminated directly regardless
/// of which engine it is, and never gets a CDP `Browser.close` (the process
/// exiting takes the CDP endpoint with it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TeardownAction {
    /// `child: Some(_)` — a locally-spawned process this session owns.
    /// SIGTERM, bounded grace, hard kill, reap. Never `Browser.close`.
    TerminateChild,
    /// `child: None`, `backend: Chromium` — chromiumoxide/the OS owns the
    /// process; send CDP `Browser.close` so it exits cleanly, as before.
    SendBrowserClose,
    /// `child: None`, `backend: Obscura` — a remote endpoint this process
    /// did not start (built via `connect()`). Drop the connection only;
    /// never send `Browser.close` to a server we do not own.
    DropOnly,
}

/// See [`TeardownAction`]. `has_child` always wins: a locally-spawned process
/// is terminated directly no matter which `backend` spawned it.
fn teardown_action(has_child: bool, backend: BrowserBackend) -> TeardownAction {
    if has_child {
        TeardownAction::TerminateChild
    } else {
        match backend {
            BrowserBackend::Chromium => TeardownAction::SendBrowserClose,
            BrowserBackend::Obscura => TeardownAction::DropOnly,
        }
    }
}

/// Phase 53 Plan 03 (T-53-03-03): best-effort implicit teardown for the case
/// `close()` is never called explicitly — the `Arc<Mutex<Option<Self>>>` is
/// set to `None`, or `AgentLoop` itself drops. `Drop::drop` cannot `await`,
/// so this cannot run the graceful SIGTERM-then-wait sequence `close()`
/// does; it only `start_kill()`s a locally-spawned child on a best-effort
/// basis so an implicit drop cannot leave `obscura serve` running
/// indefinitely. It does not reap — `close()` remains the path that reaps.
impl Drop for BrowserSession {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}

/// Phase 53 (Plan 02 Task 1): shared tail of `spawn`/`connect` — starts the CDP
/// websocket pump, opens the initial `about:blank` page, and assembles the
/// session. The Chromium-launch path, the Obscura-local-spawn path, and (from
/// Plan 02 Task 3) the remote `connect` path all share this one copy rather than
/// duplicating the pump-spawn + new_page + struct-literal tail.
///
/// On failure after a `Some(child)` was passed in, the child is killed before the
/// error is returned so a failed session build leaks no process (the local-spawn
/// arm may have already run `obscura serve` before this helper is reached).
async fn build_session(
    browser: Browser,
    mut handler: Handler,
    child: Option<Child>,
    backend: BrowserBackend,
) -> anyhow::Result<BrowserSession> {
    // CDP websocket pump — must run on a separate task so handler can drive events.
    let handler_task = tokio::spawn(async move {
        while let Some(h) = handler.next().await {
            if let Err(e) = h {
                warn!(error = %e, "Phase 25.1: CDP handler error; pump exiting");
                break;
            }
        }
    });

    let page = match browser.new_page("about:blank").await {
        Ok(page) => page,
        Err(e) => {
            handler_task.abort();
            if let Some(mut child) = child {
                let _ = child.start_kill();
            }
            return Err(anyhow::anyhow!("browser new_page failed: {e}"));
        }
    };

    Ok(BrowserSession {
        browser,
        page,
        ref_table: HashMap::new(),
        console_buffer: Vec::new(),
        handler_task,
        child,
        backend,
    })
}

/// Phase 53 (D-10 resolver template): walk `OBSCURA_PATH` env var → config path →
/// inline PATH search. Mirrors `find_chromium_binary`'s D-05 resolution shape (env
/// var authoritative when set, no fall-through) but carries no hardcoded platform
/// install paths — Obscura has no equivalent of `/Applications/Google Chrome.app`.
///
/// `config_path` is the operator-set `browser.obscura_path` from config.yaml. Only
/// consulted by the caller when `backend` selects Obscura.
pub fn find_obscura_binary(config_path: Option<&str>) -> Option<PathBuf> {
    // Same test-only escape hatch as find_chromium_binary — forces None even if
    // an obscura binary is installed, so tests can deterministically reproduce
    // the "no obscura" condition.
    if std::env::var("IRONHERMES_BROWSER_TEST_DISABLE").as_deref() == Ok("1") {
        return None;
    }
    // 1. OBSCURA_PATH env var — authoritative when set (non-empty). Do NOT fall
    //    through to config/PATH when it's set but invalid — the operator's intent
    //    is explicit, mirroring BROWSER_PATH/CHROMIUM_PATH's semantics.
    if let Some(p) = std::env::var("OBSCURA_PATH").ok().filter(|s| !s.is_empty()) {
        let path = PathBuf::from(&p);
        return if path.is_file() { Some(path) } else { None };
    }
    // 2. config.browser.obscura_path
    if let Some(p) = config_path {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    // 3. Inline PATH search (no `which` crate — matches find_chromium_binary's
    //    zero-new-deps convention).
    if let Ok(path_var) = std::env::var("PATH") {
        let separator = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(separator) {
            let candidate = PathBuf::from(dir).join("obscura");
            if candidate.is_file() {
                return Some(candidate);
            }
            #[cfg(windows)]
            {
                let candidate_exe = PathBuf::from(dir).join("obscura.exe");
                if candidate_exe.is_file() {
                    return Some(candidate_exe);
                }
            }
        }
    }
    None
}

/// Phase 53 (D-03/D-04): self-bind a free ephemeral port (unless `obscura_port`
/// pins one), then spawn `obscura serve --port <N>` against it, appending
/// `--stealth`/`--allow-private-network` per config. Obscura's own `--port 0`
/// support cannot be trusted to report the bound port back — `obscura serve`
/// never calls `local_addr()` and only logs the literal port it was given
/// (RESEARCH.md Pitfall 1) — so IronHermes binds the port itself when
/// `obscura_port` is `None`, reads it back, drops the listener, and passes the
/// concrete number to `--port`. The `serve` subcommand is mandatory: Obscura's
/// no-subcommand path silently drops flags including `--allow-private-network`
/// (RESEARCH.md Pitfall 2), so this always constructs `obscura serve …`, never a
/// bare `obscura …`.
async fn spawn_obscura_serve(
    config: &BrowserConfig,
    binary: &std::path::Path,
) -> anyhow::Result<(Child, u16)> {
    let port = match config.obscura_port {
        Some(p) => p,
        None => {
            let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| {
                anyhow::anyhow!("failed to bind an ephemeral port for obscura serve: {e}")
            })?;
            listener
                .local_addr()
                .map_err(|e| {
                    anyhow::anyhow!("failed to read ephemeral port for obscura serve: {e}")
                })?
                .port()
            // listener dropped here — small TOCTOU window, acceptable for a
            // per-profile local process (D-03's own isolation rationale, not
            // adversarial hardening).
        }
    };

    let mut cmd = Command::new(binary);
    cmd.arg("serve").arg("--port").arg(port.to_string());
    for flag in obscura_serve_flags(config) {
        cmd.arg(flag);
    }

    let child = cmd.spawn().map_err(|e| BrowserStartError::SpawnFailed {
        command: format!("obscura serve --port {port}"),
        source: e,
    })?;

    Ok((child, port))
}

/// Phase 53 (D-03 `obscura_stealth` / D-04 `obscura_allow_private_network`): the
/// config-driven optional flags appended to `obscura serve`, factored out of
/// `spawn_obscura_serve` so the config→flag mapping is unit-testable without
/// spawning a process. `--allow-private-network` is a straight passthrough to
/// Obscura's own deny-by-default SSRF guard — no second allowlist is
/// implemented here (D-04).
fn obscura_serve_flags(config: &BrowserConfig) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if config.obscura_stealth {
        flags.push("--stealth");
    }
    if config.obscura_allow_private_network {
        flags.push(ALLOW_PRIVATE_NETWORK_FLAG);
    }
    flags
}

/// The single literal source of the `--allow-private-network` flag text, shared
/// with its own tests so the string appears exactly once in this file (D-04's
/// own verify gate asserts that — one passthrough site, not a second allowlist).
const ALLOW_PRIVATE_NETWORK_FLAG: &str = "--allow-private-network";

/// Phase 53 (D-03): poll a freshly-spawned `obscura serve`'s HTTP control plane
/// (`/json/version`, confirmed to exist and answer synchronously even while the
/// engine's own event loop may be busy — RESEARCH.md) until it responds or
/// `timeout` elapses, whichever comes first. Returns a named timeout error —
/// never hangs unbounded.
async fn wait_for_obscura_ready(port: u16, timeout: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    let url = format!("http://127.0.0.1:{port}/json/version");
    let client = reqwest::Client::new();
    let mut backoff = Duration::from_millis(50);
    loop {
        if let Ok(resp) = client.get(&url).send().await
            && resp.status().is_success()
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(BrowserStartError::ReadinessTimeout {
                what: format!("obscura serve on port {port} (GET {url})"),
                timeout_secs: timeout.as_secs(),
            }
            .into());
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_millis(500));
    }
}

/// Phase 25.1 D-05: walk env vars → config path → inline PATH search → platform paths.
/// Returns Some(path) when a valid chromium binary file is found, else None.
///
/// `config_path` is the operator-set `browser.chromium_path` from config.yaml (highest precedence
/// after the env vars per D-05 ordering: env vars > config > PATH > platform paths).
pub fn find_chromium_binary(config_path: Option<&str>) -> Option<PathBuf> {
    // Phase 25.1 D-21: test-only escape hatch — forces None even if chromium is installed.
    // Set IRONHERMES_BROWSER_TEST_DISABLE=1 to deterministically reproduce the "no chromium"
    // condition in browser_prereq.rs tests on dev machines with system Chrome installed.
    // This var MUST NOT be set in production environments.
    if std::env::var("IRONHERMES_BROWSER_TEST_DISABLE").as_deref() == Ok("1") {
        return None;
    }
    // 1. BROWSER_PATH env var (D-05 step 1: authoritative when set).
    //    Empty string treated as unset (POSIX convention: BROWSER_PATH= == not exported).
    //    When set to a non-empty value: return Some(path) if valid file, else return None.
    //    Do NOT fall through to subsequent sources — the operator's intent is explicit.
    if let Some(p) = std::env::var("BROWSER_PATH").ok().filter(|s| !s.is_empty()) {
        let path = PathBuf::from(&p);
        return if path.is_file() { Some(path) } else { None };
    }
    // 2. CHROMIUM_PATH env var (D-05 step 2: authoritative when set).
    //    Same authoritative semantics as BROWSER_PATH above.
    if let Some(p) = std::env::var("CHROMIUM_PATH")
        .ok()
        .filter(|s| !s.is_empty())
    {
        let path = PathBuf::from(&p);
        return if path.is_file() { Some(path) } else { None };
    }
    // 3. config.browser.chromium_path
    if let Some(p) = config_path {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    // 4. Inline PATH search (no `which` crate per OQ-4 — zero new workspace deps).
    if let Ok(path_var) = std::env::var("PATH") {
        let separator = if cfg!(windows) { ';' } else { ':' };
        for name in &["chromium-browser", "chromium", "google-chrome", "chrome"] {
            for dir in path_var.split(separator) {
                let candidate = PathBuf::from(dir).join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
                #[cfg(windows)]
                {
                    let candidate_exe = PathBuf::from(dir).join(format!("{name}.exe"));
                    if candidate_exe.is_file() {
                        return Some(candidate_exe);
                    }
                }
            }
        }
    }
    // 5. macOS platform paths
    #[cfg(target_os = "macos")]
    for p in &[
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ] {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    // 6. Linux platform paths
    #[cfg(target_os = "linux")]
    for p in &[
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/usr/bin/google-chrome",
        "/snap/bin/chromium",
    ] {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    // 7. Windows platform paths
    #[cfg(target_os = "windows")]
    for p in &[
        "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
        "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
        "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
    ] {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Phase 53 Plan 04 (D-09/D-10): the single answer to "is a browser engine
/// available here" — reads the FULL `BrowserConfig` (`backend`,
/// `chromium_path`, `obscura_path`, `cdp_url`) so this finally agrees with
/// what `spawn()` would actually do. Dispatches on `config.backend` FIRST,
/// mirroring `spawn`'s D-01 authority rule, so the two can never disagree:
///
///   * `Obscura`: available when `cdp_url` is set (remote mode needs no
///     local engine), otherwise when `find_obscura_binary` resolves
///     `config.obscura_path`.
///   * `Chromium`: available when `find_chromium_binary` resolves
///     `config.chromium_path`. `cdp_url` is NOT consulted here — it is
///     inert under `backend: chromium` (D-01), exactly as in `spawn`.
///
/// Passing `config.chromium_path`/`config.obscura_path` through (rather than
/// the pre-53 eleven call sites' hardcoded `None`) is the D-10 fix: an
/// operator whose engine lives only at a config-set custom path is no
/// longer hidden from the model.
///
/// Deliberately a free function, not a registry `Prerequisite` group: a
/// `binary_present` prerequisite kind falls through
/// `prerequisite_satisfied`'s `_ => true` arm (`registry.rs:114`), so an
/// any-of group of two binary prerequisites would report both satisfied and
/// gate nothing. This function — and the eleven `browser_*` tools'
/// `is_available()` delegating to it directly — is the real gate.
///
/// Performs only path and env lookups — no process spawn, no network call.
/// Safe to call for every tool at registry-construction time (T-53-04-03).
pub fn configured_browser_engine_available(config: &BrowserConfig) -> bool {
    let available = match config.backend {
        BrowserBackend::Obscura => {
            if config.cdp_url.is_some() {
                true
            } else {
                find_obscura_binary(config.obscura_path.as_deref()).is_some()
            }
        }
        // D-01: cdp_url is NOT consulted on this arm — inert under Chromium.
        BrowserBackend::Chromium => {
            find_chromium_binary(config.chromium_path.as_deref()).is_some()
        }
    };
    if !available {
        // D-09: a tool vanishing from the model's schema is otherwise
        // invisible — loud, not silent (registry.rs's config_field
        // precedent, :134-149). Name the backend and the dotted setting
        // that would fix it — never a path value the operator did not
        // already put in their own config.
        let setting = match config.backend {
            BrowserBackend::Obscura => "browser.obscura_path",
            BrowserBackend::Chromium => "browser.chromium_path",
        };
        tracing::warn!(
            backend = ?config.backend,
            setting,
            "no browser engine binary discoverable for the configured backend — \
             all browser_* tools are being removed from the model's schema"
        );
    }
    available
}

/// Phase 53 Plan 04 (D-09/D-10): the `binary_present` [`Prerequisite`]
/// describing whichever engine `config.backend` selects. The Chromium arm
/// reproduces the pre-53 `chromium-or-chrome` entry verbatim — the same
/// text ten call sites used to hand-roll identically. The Obscura arm names
/// `browser.obscura_path` and the `--features render` build requirement.
pub fn configured_engine_prerequisite(config: &BrowserConfig) -> Prerequisite {
    match config.backend {
        BrowserBackend::Chromium => Prerequisite {
            kind: "binary_present".to_string(),
            name: "chromium-or-chrome".to_string(),
            description:
                "Chromium or Google Chrome browser binary on PATH or at a standard install location"
                    .to_string(),
            required: true,
            group: None,
        },
        BrowserBackend::Obscura => Prerequisite {
            kind: "binary_present".to_string(),
            name: "obscura".to_string(),
            description: "Obscura binary (built with `--features render`) on PATH, at \
                           browser.obscura_path, or reachable via browser.cdp_url"
                .to_string(),
            required: true,
            group: None,
        },
    }
}

/// Phase 53 Plan 03 (Task 2, D-05): confirm an Obscura build can actually lay
/// out and paint a page before handing a session back to any `browser_*`
/// tool. Called at the end of BOTH Obscura construction paths — `spawn`'s
/// local-spawn arm, and `connect` (which also covers `spawn`'s remote
/// `cdp_url` arm, since it delegates to `connect`) — and on neither Chromium
/// path. Module-scope, not a `BrowserSession` method: a private helper both
/// call sites reach directly.
///
/// Requires BOTH legs to agree, after navigating to a minimal self-contained
/// fixture document (never over the network — a `data:text/html` URL, so
/// this cannot become an SSRF surface reachable at every session start):
///   * Leg 1 — `Page.captureScreenshot` succeeds. Obscura's own CDP handler
///     `#[cfg(not(feature = "render"))]`-gates this method to fail
///     unconditionally, so a failure here after a confirmed navigation is a
///     strong, protocol-level signal — but the SAME match arm also returns
///     ordinary not-ready errors on a genuine render build ("no retained
///     document size", "the page has no DOM to render"), so leg 1 alone is
///     not sufficient. Treat ANY error as a failure of this leg — never
///     match on the error text, which could change upstream without notice.
///   * Leg 2 — `getComputedStyle` on an author-styled `display:none`
///     element reports `display: none`. This is exactly the property
///     `browser_snapshot.rs`'s visibility walker consumes
///     (`window.getComputedStyle(el)` + `style.display === 'none'`), not an
///     approximation of it. Defensively defaults to `false` (never a pass)
///     if the value is absent or evaluation errors.
///
/// D-05: no config field, env var, or boolean parameter can bypass this
/// check — the call sites take no such parameter, and `BrowserConfig` gains
/// no field in this plan.
async fn probe_render_support(page: &Page) -> Result<(), BrowserStartError> {
    const FIXTURE: &str = "data:text/html,\
        <div id=\"ironhermes-probe-visible\">hi</div>\
        <div id=\"ironhermes-probe-hidden\" style=\"display:none\">bye</div>";

    // Navigate to real content first (RESEARCH.md Pitfall 3) — running
    // either leg against about:blank would make both a false positive and a
    // false negative equally likely.
    let _ = page.goto(FIXTURE).await;

    let screenshot_ok = page
        .screenshot(chromiumoxide::page::ScreenshotParams::builder().build())
        .await
        .is_ok();

    let hidden_reports_none: bool = page
        .evaluate(
            "window.getComputedStyle(document.getElementById('ironhermes-probe-hidden')) \
             .display === 'none'",
        )
        .await
        .ok()
        .and_then(|v| v.into_value().ok())
        .unwrap_or(false);

    let outcome = render_probe_outcome(screenshot_ok, hidden_reports_none);
    if outcome.is_ok() {
        // Restore the contract spawn()/connect() promise: a fresh
        // about:blank page (browser_session.rs:45-46).
        let _ = page.goto("about:blank").await;
    }
    outcome
}

/// The two-leg decision `probe_render_support` acts on, factored out so it
/// is unit-testable without a live CDP connection. Both legs are required —
/// the refusal condition is a logical OR of the two negations, never a
/// single check.
fn render_probe_outcome(
    screenshot_ok: bool,
    hidden_reports_none: bool,
) -> Result<(), BrowserStartError> {
    if screenshot_ok && hidden_reports_none {
        Ok(())
    } else {
        Err(BrowserStartError::RenderUnsupported {
            screenshot_ok,
            hidden_reports_none,
        })
    }
}

/// Phase 53 Plan 03 (Task 3, D-06): the result of running the render probe
/// standalone via `ironhermes doctor --browser`, without going through a
/// full agent turn.
#[derive(Debug)]
pub struct BrowserDiagnosis {
    /// The configured backend (Chromium or Obscura).
    pub backend: BrowserBackend,
    /// Human-readable mode implied by this configuration: "Chromium",
    /// "Obscura (local)", or "Obscura (remote)".
    pub mode: &'static str,
    /// The resolved binary path for a locally-spawned engine. `None` for
    /// Obscura remote mode (no local binary to resolve) or when resolution
    /// failed.
    pub binary_path: Option<PathBuf>,
    /// The outcome of actually spawning a session against this config.
    pub outcome: BrowserDiagnosisOutcome,
}

/// See [`BrowserDiagnosis::outcome`].
#[derive(Debug)]
pub enum BrowserDiagnosisOutcome {
    /// A session was spawned and the render probe (when applicable) passed.
    Ok,
    /// D-05: the render probe refused this build by name.
    RenderUnsupported {
        screenshot_ok: bool,
        hidden_reports_none: bool,
    },
    /// Any other spawn failure (binary not found, spawn/readiness/connect
    /// failure). The message is the underlying error's own Display text, so
    /// it already names what went wrong.
    Failed(String),
}

/// Phase 53 Plan 03 (Task 3, D-06): run the render probe standalone against
/// the configured backend, going through the SAME `BrowserSession::spawn`
/// path the runtime uses — the probe `doctor --browser` reports is the same
/// probe the runtime enforces, with no second copy to drift. Never panics
/// and never leaks a child process: `close()`s whatever it gets on every
/// path (success or failure — `spawn`'s own failure paths already terminate
/// a partially-constructed local child before returning `Err`, so there is
/// nothing left to close on failure here).
pub async fn diagnose(config: &BrowserConfig) -> BrowserDiagnosis {
    let backend = config.backend.clone();
    let mode = match (&backend, config.cdp_url.is_some()) {
        (BrowserBackend::Chromium, _) => "Chromium",
        (BrowserBackend::Obscura, true) => "Obscura (remote)",
        (BrowserBackend::Obscura, false) => "Obscura (local)",
    };

    let binary_path = match &backend {
        BrowserBackend::Chromium => find_chromium_binary(config.chromium_path.as_deref()),
        BrowserBackend::Obscura if config.cdp_url.is_some() => None,
        BrowserBackend::Obscura => find_obscura_binary(config.obscura_path.as_deref()),
    };

    let outcome = match BrowserSession::spawn(config).await {
        Ok(session) => {
            let _ = session.close().await;
            BrowserDiagnosisOutcome::Ok
        }
        Err(e) => match e.downcast_ref::<BrowserStartError>() {
            Some(BrowserStartError::RenderUnsupported {
                screenshot_ok,
                hidden_reports_none,
            }) => BrowserDiagnosisOutcome::RenderUnsupported {
                screenshot_ok: *screenshot_ok,
                hidden_reports_none: *hidden_reports_none,
            },
            _ => BrowserDiagnosisOutcome::Failed(e.to_string()),
        },
    };

    BrowserDiagnosis {
        backend,
        mode,
        binary_path,
        outcome,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    fn env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    #[test]
    fn find_chromium_binary_returns_none_when_browser_path_set_to_absent_file_strict() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: env_lock + --test-threads=1 ensure single mutator (Phase 21.6 Rust 2024 pattern).
        unsafe {
            std::env::set_var("BROWSER_PATH", "/dev/null/definitely-absent-chromium");
            std::env::remove_var("CHROMIUM_PATH");
            std::env::set_var("PATH", "/dev/null/empty-path");
        }
        // GAP-2 regression test: BROWSER_PATH authoritative when set (D-05 step 1).
        // Must return None even on dev machines with system Chrome installed.
        let result = find_chromium_binary(None);
        assert!(
            result.is_none(),
            "BROWSER_PATH set but invalid → must return None (not fall through to system Chrome), got {:?}",
            result
        );
        unsafe {
            std::env::remove_var("BROWSER_PATH");
            std::env::remove_var("PATH");
        }
    }

    #[test]
    fn find_chromium_binary_returns_none_when_chromium_path_set_to_absent_file_strict() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: env_lock + --test-threads=1 ensure single mutator (Phase 21.6 Rust 2024 pattern).
        unsafe {
            std::env::remove_var("BROWSER_PATH");
            std::env::set_var("CHROMIUM_PATH", "/dev/null/definitely-absent-chromium");
            std::env::set_var("PATH", "/dev/null/empty-path");
        }
        // GAP-2 regression test: CHROMIUM_PATH authoritative when set (D-05 step 2).
        // BROWSER_PATH is unset so falls through to CHROMIUM_PATH, which is set but invalid.
        // Must return None even on dev machines with system Chrome installed.
        let result = find_chromium_binary(None);
        assert!(
            result.is_none(),
            "CHROMIUM_PATH set but invalid → must return None (not fall through to system Chrome), got {:?}",
            result
        );
        unsafe {
            std::env::remove_var("CHROMIUM_PATH");
            std::env::remove_var("PATH");
        }
    }

    #[test]
    fn find_chromium_binary_falls_through_when_browser_path_unset() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: env_lock + --test-threads=1 ensure single mutator (Phase 21.6 Rust 2024 pattern).
        unsafe {
            std::env::remove_var("BROWSER_PATH");
            std::env::remove_var("CHROMIUM_PATH");
        }
        // When both env vars are UNSET, the function falls through to step 3 (config_path).
        // Using /bin/sh as a stand-in for a real chromium binary — tests path resolution,
        // not whether the binary IS chromium.
        #[cfg(unix)]
        {
            let result = find_chromium_binary(Some("/bin/sh"));
            assert_eq!(
                result,
                Some(PathBuf::from("/bin/sh")),
                "When env vars unset, config_path (/bin/sh) must be returned (fall-through preserved)"
            );
        }
    }

    #[test]
    fn find_chromium_binary_uses_browser_path_when_set_to_real_file() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // Use /bin/sh (always exists on macOS+Linux) as a stand-in for a chromium binary —
        // we're testing the path-resolution contract, not whether the binary IS chromium.
        #[cfg(unix)]
        {
            let real_file = "/bin/sh";
            unsafe {
                std::env::set_var("BROWSER_PATH", real_file);
            }
            let result = find_chromium_binary(None);
            unsafe {
                std::env::remove_var("BROWSER_PATH");
            }
            assert_eq!(
                result,
                Some(PathBuf::from(real_file)),
                "BROWSER_PATH pointing at a real file MUST be returned"
            );
        }
        #[cfg(not(unix))]
        {
            // Skip on non-unix — no guaranteed real file path
        }
    }

    // -------------------------------------------------------------------
    // Phase 53 Plan 04 (D-09/D-10): configured_browser_engine_available /
    // configured_engine_prerequisite
    // -------------------------------------------------------------------

    #[test]
    fn chromium_at_a_config_only_custom_path_is_available() {
        // D-10 regression (the headline case): an operator whose Chromium
        // lives only at a config-set custom path must see the browser
        // toolset. The pre-53 bug was every one of the eleven call sites
        // passing `None` to `find_chromium_binary`, discarding
        // `browser.chromium_path` entirely.
        #[cfg(unix)]
        {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            unsafe {
                std::env::remove_var("BROWSER_PATH");
                std::env::remove_var("CHROMIUM_PATH");
                std::env::remove_var("IRONHERMES_BROWSER_TEST_DISABLE");
            }
            let config = BrowserConfig {
                backend: BrowserBackend::Chromium,
                chromium_path: Some("/bin/sh".to_string()),
                ..Default::default()
            };

            // The fix: configured_browser_engine_available threads
            // config.chromium_path through to find_chromium_binary, so step 3
            // (config path) resolves the custom-path binary regardless of PATH
            // or platform-default install locations.
            let available = configured_browser_engine_available(&config);

            // The bug, demonstrated directly against the identical config's
            // path: find_chromium_binary(None) — the pre-53 call shape every
            // one of the eleven tools used — discards chromium_path entirely.
            // Scope the IRONHERMES_BROWSER_TEST_DISABLE escape hatch to ONLY
            // this call so the assertion is deterministic on a dev machine
            // that happens to have a real Chrome install at a macOS/Linux/
            // Windows platform-default path (D-05 steps 5-7) — without it, a
            // real system Chrome could mask the very bug this test pins.
            unsafe {
                std::env::set_var("IRONHERMES_BROWSER_TEST_DISABLE", "1");
            }
            let legacy_bug_result = find_chromium_binary(None);
            unsafe {
                std::env::remove_var("IRONHERMES_BROWSER_TEST_DISABLE");
            }

            assert!(
                available,
                "configured_browser_engine_available must find chromium at the \
                 config-only custom path"
            );
            assert!(
                legacy_bug_result.is_none(),
                "find_chromium_binary(None) — the pre-53 call shape — must NOT see \
                 the config-only custom path; this is the D-10 bug being pinned"
            );
        }
    }

    #[test]
    fn availability_follows_the_configured_backend_not_the_chromium_binary() {
        #[cfg(unix)]
        {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            unsafe {
                std::env::remove_var("OBSCURA_PATH");
            }
            let config = BrowserConfig {
                backend: BrowserBackend::Obscura,
                obscura_path: Some("/bin/sh".to_string()),
                ..Default::default()
            };
            // Deliberately does NOT touch BROWSER_PATH/CHROMIUM_PATH/PATH —
            // this machine may have a real Chromium installed, and that must
            // be irrelevant: configured_browser_engine_available dispatches
            // on config.backend FIRST (D-01) and never falls back to
            // Chromium.
            let available = configured_browser_engine_available(&config);
            unsafe {
                std::env::remove_var("OBSCURA_PATH");
            }
            assert!(
                available,
                "backend: obscura with a resolvable obscura binary must be \
                 available regardless of whether Chromium is also installed"
            );
        }
    }

    #[test]
    fn a_cdp_url_under_obscura_makes_availability_independent_of_any_local_binary()
     {
        let config = BrowserConfig {
            backend: BrowserBackend::Obscura,
            cdp_url: Some("ws://127.0.0.1:9/not-a-real-obscura".to_string()),
            obscura_path: None,
            ..Default::default()
        };
        // cdp_url short-circuits before find_obscura_binary is ever called —
        // no local binary needed, matching spawn()'s own cdp_url.is_some()
        // branch.
        assert!(
            configured_browser_engine_available(&config),
            "backend: obscura with cdp_url set must be available with no local \
             binary"
        );
    }

    #[test]
    fn a_cdp_url_under_chromium_does_not_confer_availability() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            std::env::set_var("IRONHERMES_BROWSER_TEST_DISABLE", "1");
        }
        let config = BrowserConfig {
            backend: BrowserBackend::Chromium,
            cdp_url: Some("ws://127.0.0.1:9/not-a-real-obscura".to_string()),
            ..Default::default()
        };
        let available = configured_browser_engine_available(&config);
        unsafe {
            std::env::remove_var("IRONHERMES_BROWSER_TEST_DISABLE");
        }
        assert!(
            !available,
            "D-01: cdp_url is inert under backend: chromium — must not confer \
             availability with no chromium binary discoverable"
        );
    }

    #[test]
    fn chromium_backend_still_requires_a_chromium_binary() {
        // Today's behaviour, re-pinned: with no chromium_path override, the
        // resolver's answer must track find_chromium_binary exactly, whatever
        // this machine's actual answer is (present or absent) — the point is
        // parity, not a specific outcome.
        let config = BrowserConfig::default();
        assert_eq!(
            configured_browser_engine_available(&config),
            find_chromium_binary(config.chromium_path.as_deref()).is_some(),
            "backend: chromium (default) must track find_chromium_binary exactly"
        );
    }

    #[test]
    fn no_engine_and_no_cdp_url_is_unavailable() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            std::env::set_var("IRONHERMES_BROWSER_TEST_DISABLE", "1");
        }
        let chromium_cfg = BrowserConfig {
            backend: BrowserBackend::Chromium,
            ..Default::default()
        };
        let obscura_cfg = BrowserConfig {
            backend: BrowserBackend::Obscura,
            ..Default::default()
        };
        let chromium_available = configured_browser_engine_available(&chromium_cfg);
        let obscura_available = configured_browser_engine_available(&obscura_cfg);
        unsafe {
            std::env::remove_var("IRONHERMES_BROWSER_TEST_DISABLE");
        }
        assert!(
            !chromium_available,
            "no engine + no cdp_url must be unavailable under backend: chromium"
        );
        assert!(
            !obscura_available,
            "no engine + no cdp_url must be unavailable under backend: obscura"
        );
    }

    #[test]
    #[tracing_test::traced_test]
    fn an_unavailable_backend_emits_a_loud_warn_naming_the_setting() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            std::env::set_var("IRONHERMES_BROWSER_TEST_DISABLE", "1");
        }
        let config = BrowserConfig {
            backend: BrowserBackend::Obscura,
            ..Default::default()
        };
        let available = configured_browser_engine_available(&config);
        unsafe {
            std::env::remove_var("IRONHERMES_BROWSER_TEST_DISABLE");
        }
        assert!(!available);
        assert!(
            logs_contain("browser.obscura_path"),
            "D-09 warn must name the dotted setting that would fix it"
        );
        assert!(
            logs_contain("Obscura"),
            "D-09 warn must name the configured backend"
        );
    }

    #[test]
    fn the_resolver_performs_no_process_spawn_and_no_network_call() {
        let src = include_str!("browser_session.rs");
        let sig_needle = format!(
            "{}{}",
            "pub fn configured_browser_engine_", "available"
        );
        let start = src
            .find(&sig_needle)
            .expect("configured_browser_engine_available must exist");
        let body_start = src[start..]
            .find('{')
            .map(|i| start + i)
            .expect("function must have a body");
        let end = src[body_start..]
            .find("\n}\n")
            .map(|i| body_start + i)
            .expect("function body must close");
        let body = &src[body_start..end];
        for forbidden in [
            "Config::load",
            "Command",
            "::spawn(",
            "::connect(",
            "reqwest",
            "http://",
            "https://",
        ] {
            assert!(
                !body.contains(forbidden),
                "configured_browser_engine_available must perform no process \
                 spawn or network call — found forbidden token {forbidden:?}"
            );
        }
    }

    #[test]
    fn the_prerequisite_names_the_configured_engine() {
        let obscura_cfg = BrowserConfig {
            backend: BrowserBackend::Obscura,
            ..Default::default()
        };
        let p = configured_engine_prerequisite(&obscura_cfg);
        assert_eq!(p.kind, "binary_present");
        assert!(
            p.name.contains("obscura"),
            "obscura prerequisite name must name obscura, got {:?}",
            p.name
        );
        assert!(p.description.contains("browser.obscura_path"));
        assert!(p.description.contains("--features render"));

        let chromium_cfg = BrowserConfig {
            backend: BrowserBackend::Chromium,
            ..Default::default()
        };
        let p2 = configured_engine_prerequisite(&chromium_cfg);
        assert_eq!(p2.kind, "binary_present");
        assert_eq!(p2.name, "chromium-or-chrome");
        assert_eq!(
            p2.description,
            "Chromium or Google Chrome browser binary on PATH or at a standard \
             install location"
        );
    }

    #[test]
    fn validate_navigation_url_empty_allowlist_allows_all() {
        assert!(BrowserSession::validate_navigation_url(&[], "https://example.com").is_ok());
        assert!(BrowserSession::validate_navigation_url(&[], "http://internal.local").is_ok());
    }

    #[test]
    fn validate_navigation_url_allowlist_blocks_unlisted_host() {
        let allow = vec!["example.com".to_string()];
        let result = BrowserSession::validate_navigation_url(&allow, "https://evil.com");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("domain_blocked"));
        assert!(msg.contains("evil.com"));
        assert!(msg.contains("example.com"));
    }

    #[test]
    fn validate_navigation_url_allowlist_allows_listed_host() {
        let allow = vec!["example.com".to_string()];
        assert!(
            BrowserSession::validate_navigation_url(&allow, "https://example.com/page").is_ok()
        );
    }

    // =========================================================================
    // Phase 53 (Plan 02 Task 1): Obscura tracer tests
    // =========================================================================

    #[test]
    fn browser_backend_defaults_to_chromium() {
        assert_eq!(BrowserConfig::default().backend, BrowserBackend::Chromium);
    }

    #[tokio::test]
    async fn a_missing_obscura_binary_fails_with_a_named_error() {
        // Guard scope ends before the await below — clippy::await_holding_lock
        // forbids holding a std::sync::MutexGuard across an .await point; the
        // --test-threads=1 discipline (Phase 21.6 pattern) is what actually
        // serializes env-var mutation across test functions.
        {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            // SAFETY: env_lock + --test-threads=1 ensure single mutator.
            unsafe {
                std::env::set_var("IRONHERMES_BROWSER_TEST_DISABLE", "1");
            }
        }
        let config = BrowserConfig {
            backend: BrowserBackend::Obscura,
            ..Default::default()
        };
        let result = BrowserSession::spawn(&config).await;
        {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            unsafe {
                std::env::remove_var("IRONHERMES_BROWSER_TEST_DISABLE");
            }
        }
        // BrowserSession doesn't implement Debug (chromiumoxide::Browser doesn't),
        // so expect_err/unwrap_err (which require T: Debug) aren't usable here.
        let err = match result {
            Ok(_) => panic!("spawn must fail when no obscura binary resolves"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.to_lowercase().contains("obscura"),
            "error must name obscura, got: {err}"
        );
        assert!(
            err.contains("browser.obscura_path"),
            "error must name the setting to fix, got: {err}"
        );
    }

    #[tokio::test]
    async fn obscura_readiness_polling_gives_up_rather_than_hanging() {
        // Bind an ephemeral port, then drop the listener immediately — nothing
        // will ever answer GET /json/version on it.
        let port = {
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
            listener.local_addr().expect("local_addr").port()
        };
        let start = std::time::Instant::now();
        let result = wait_for_obscura_ready(port, Duration::from_millis(300)).await;
        let elapsed = start.elapsed();
        assert!(
            result.is_err(),
            "readiness poll must time out, not hang, against a dead port"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "readiness poll must be bounded by the configured timeout, took {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn obscura_local_spawn_binds_an_ephemeral_port_not_a_fixed_one() {
        // Uses /bin/sh as a stand-in child process — this test exercises D-03's
        // port-allocation contract, not whether the spawned program IS obscura.
        #[cfg(unix)]
        {
            let sh = std::path::Path::new("/bin/sh");
            let config = BrowserConfig::default();
            let (mut child1, port1) = spawn_obscura_serve(&config, sh)
                .await
                .expect("first spawn_obscura_serve must succeed");
            let (mut child2, port2) = spawn_obscura_serve(&config, sh)
                .await
                .expect("second spawn_obscura_serve must succeed");
            assert_ne!(
                port1, port2,
                "two consecutive local spawns must receive two different ports (D-03)"
            );
            let _ = child1.start_kill();
            let _ = child2.start_kill();
            let _ = child1.wait().await;
            let _ = child2.wait().await;
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tracer_obscura_local_spawn_loads_a_real_page() {
        // Read-only w.r.t. shared env state (only reads OBSCURA_PATH, set once by
        // the harness before the process starts) — no env_lock() needed here.
        let binary = match find_obscura_binary(None) {
            Some(b) => b,
            None => {
                eprintln!(
                    "SKIP tracer_obscura_local_spawn_loads_a_real_page: no obscura binary \
                     (set OBSCURA_PATH or browser.obscura_path)"
                );
                return;
            }
        };

        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/tracer-page"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
                r#"<!doctype html><html><head><title>Obscura Tracer Page</title></head><body><h1>hi</h1></body></html>"#,
                "text/html; charset=utf-8",
            ))
            .mount(&server)
            .await;

        let config = BrowserConfig {
            backend: BrowserBackend::Obscura,
            obscura_path: Some(binary.to_string_lossy().into_owned()),
            ..Default::default()
        };

        // TEST-ONLY allowance for the wiremock server's loopback address. Obscura
        // denies loopback/RFC1918/link-local by default (D-04's deny-by-default
        // guard, verified live by this very test on the first run without this
        // var: "Access to private/internal IP address 127.0.0.1 is not allowed").
        // This does NOT touch spawn_obscura_serve's production argv — D-04's
        // `obscura_allow_private_network` config field and its `--allow-private-
        // network` wiring are Task 2's. `OBSCURA_ALLOW_PRIVATE_NETWORK` is
        // Obscura's own env-var equivalent of that flag (confirmed via `obscura
        // serve --help`), inherited by the spawned child because
        // spawn_obscura_serve's Command never clears the parent environment.
        {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            // SAFETY: env_lock + --test-threads=1 ensure single mutator.
            unsafe {
                std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
            }
        }

        let spawn_result = BrowserSession::spawn(&config).await;
        {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            unsafe {
                std::env::remove_var("OBSCURA_ALLOW_PRIVATE_NETWORK");
            }
        }
        let mut session = spawn_result
            .expect("BrowserSession::spawn must succeed against a render-capable obscura binary");

        let nav_url = format!("{}/tracer-page", server.uri());
        session
            .page
            .goto(nav_url)
            .await
            .expect("navigate to tracer page must succeed");

        // Give Obscura a brief moment to finish loading before reading the title.
        tokio::time::sleep(Duration::from_millis(500)).await;

        let title = session
            .page
            .get_title()
            .await
            .expect("get_title must succeed");

        // Kill the locally-spawned obscura process ourselves — BrowserSession::close()'s
        // per-mode SIGTERM-then-kill teardown for an owned child is Plan 03's; this
        // tracer only needs to leave no leaked process behind (this plan's own
        // <verify> greps for one).
        if let Some(child) = session.child.as_mut() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        let _ = session.close().await;

        assert_eq!(
            title.as_deref(),
            Some("Obscura Tracer Page"),
            "expected the wiremock-hosted document's real title to come back through Obscura"
        );
    }

    // =========================================================================
    // Phase 53 (Plan 02 Task 2): backend authority (D-01), inert-cdp_url warn
    // (D-02), SSRF flag passthrough (D-04)
    // =========================================================================

    #[tokio::test]
    async fn a_cdp_url_is_inert_under_backend_chromium() {
        {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            // SAFETY: env_lock + --test-threads=1 ensure single mutator.
            unsafe {
                std::env::set_var("IRONHERMES_BROWSER_TEST_DISABLE", "1");
            }
        }
        let config = BrowserConfig {
            backend: BrowserBackend::Chromium,
            cdp_url: Some("ws://127.0.0.1:9/not-obscura".to_string()),
            ..Default::default()
        };
        let result = BrowserSession::spawn(&config).await;
        {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            unsafe {
                std::env::remove_var("IRONHERMES_BROWSER_TEST_DISABLE");
            }
        }
        let err = match result {
            Ok(_) => panic!(
                "spawn must fail (IRONHERMES_BROWSER_TEST_DISABLE forces no chromium binary)"
            ),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("Chromium/Chrome binary not found"),
            "backend: chromium with cdp_url set must still take the Chromium path (proving \
             cdp_url is inert, D-01), got: {err}"
        );
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn an_inert_cdp_url_emits_exactly_one_warn() {
        {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            // SAFETY: env_lock + --test-threads=1 ensure single mutator.
            unsafe {
                std::env::set_var("IRONHERMES_BROWSER_TEST_DISABLE", "1");
            }
        }
        let config = BrowserConfig {
            backend: BrowserBackend::Chromium,
            cdp_url: Some("ws://127.0.0.1:9/not-obscura".to_string()),
            ..Default::default()
        };
        let _ = BrowserSession::spawn(&config).await;
        {
            let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            unsafe {
                std::env::remove_var("IRONHERMES_BROWSER_TEST_DISABLE");
            }
        }
        assert!(
            logs_contain("browser.cdp_url"),
            "D-02 warn must name browser.cdp_url"
        );
        assert!(
            logs_contain("browser.backend"),
            "D-02 warn must name browser.backend as the setting that would honor it"
        );
        logs_assert(|lines: &[&str]| {
            let matches = lines
                .iter()
                .filter(|l| l.contains("browser.cdp_url"))
                .count();
            if matches == 1 {
                Ok(())
            } else {
                Err(format!(
                    "expected exactly one browser.cdp_url warn line, got {matches}: {lines:?}"
                ))
            }
        });
    }

    #[test]
    fn private_network_flag_is_omitted_by_default() {
        let config = BrowserConfig::default();
        let flags = obscura_serve_flags(&config);
        assert!(
            !flags.contains(&ALLOW_PRIVATE_NETWORK_FLAG),
            "default (false) must NOT append --allow-private-network, got: {flags:?}"
        );
    }

    #[test]
    fn private_network_flag_is_passed_when_enabled() {
        let config = BrowserConfig {
            obscura_allow_private_network: true,
            ..Default::default()
        };
        let flags = obscura_serve_flags(&config);
        assert!(
            flags.contains(&ALLOW_PRIVATE_NETWORK_FLAG),
            "obscura_allow_private_network: true must append --allow-private-network, got: {flags:?}"
        );
    }

    #[test]
    fn obscura_stealth_flag_is_passed_when_enabled() {
        let config = BrowserConfig {
            obscura_stealth: true,
            ..Default::default()
        };
        let flags = obscura_serve_flags(&config);
        assert!(
            flags.contains(&"--stealth"),
            "obscura_stealth: true must append --stealth, got: {flags:?}"
        );
    }

    #[tokio::test]
    async fn an_explicit_obscura_port_pins_the_port() {
        // Uses /bin/sh as a stand-in child process — this test exercises D-03's
        // port-pinning contract, not whether the spawned program IS obscura.
        #[cfg(unix)]
        {
            let sh = std::path::Path::new("/bin/sh");
            let config = BrowserConfig {
                obscura_port: Some(54321),
                ..Default::default()
            };
            let (mut child, port) = spawn_obscura_serve(&config, sh)
                .await
                .expect("spawn_obscura_serve must succeed");
            assert_eq!(
                port, 54321,
                "an explicit obscura_port must be used verbatim, never overridden by an \
                 ephemeral bind"
            );
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }

    // =========================================================================
    // Phase 53 (Plan 02 Task 3): BrowserSession::connect + the Obscura remote arm
    // =========================================================================

    #[tokio::test(flavor = "multi_thread")]
    async fn a_session_built_by_connect_owns_no_child_process() {
        let binary = match find_obscura_binary(None) {
            Some(b) => b,
            None => {
                eprintln!(
                    "SKIP a_session_built_by_connect_owns_no_child_process: no obscura binary"
                );
                return;
            }
        };
        let config = BrowserConfig::default();
        let (mut helper_child, port) = spawn_obscura_serve(&config, &binary)
            .await
            .expect("helper spawn_obscura_serve must succeed to stand up a server");
        wait_for_obscura_ready(port, Duration::from_secs(10))
            .await
            .expect("helper server must become ready");

        let ws_url = format!("ws://127.0.0.1:{port}");
        let session = BrowserSession::connect(&ws_url)
            .await
            .expect("connect must succeed against a server this test itself started");

        assert!(
            session.child.is_none(),
            "a session built via connect() must never record ownership of a child process, \
             even though this test's own helper code started the server it attaches to"
        );

        let _ = session.close().await;
        let _ = helper_child.start_kill();
        let _ = helper_child.wait().await;
    }

    /// Self-referential source-shape test: slices `connect`'s own body out of this
    /// file's source text and asserts it performs none of the Chromium-only /
    /// resolver work. Bounded to `connect`'s function body specifically (not the
    /// whole file) so it cannot match this very test's own assertion strings.
    #[test]
    fn connect_does_no_binary_discovery() {
        let source = include_str!("browser_session.rs");
        // Built from non-contiguous parts (Phase 53-01 pattern): include_str! sees
        // this test's own source too, so a literal needle would self-match and the
        // Plan-level CONNECT_FN==1 structural gate would count this line as a
        // second `pub async fn connect(` occurrence.
        let needle = format!("{}{}", "pub async fn ", "connect(");
        let start = source
            .find(&needle)
            .expect("connect fn must exist in this file");
        let after_start = &source[start..];
        let end_marker = "\n    }\n";
        let end = after_start
            .find(end_marker)
            .expect("connect fn must have a closing brace at 4-space indent")
            + end_marker.len();
        let body = &after_start[..end];
        for needle in [
            "find_chromium_binary",
            "find_obscura_binary",
            "user_data_dir",
            "reconcile_singleton_lock",
            "Command::new",
        ] {
            assert!(
                !body.contains(needle),
                "connect() must perform no binary discovery and no Chromium profile work; \
                 found '{needle}' in its body:\n{body}"
            );
        }
    }

    #[tokio::test]
    async fn a_cdp_url_under_backend_obscura_spawns_nothing() {
        // Set obscura_path too (when resolvable) to prove cdp_url wins even when
        // local-spawn WOULD have been possible (D-01).
        let obscura_binary = find_obscura_binary(None);
        let cdp_url = "ws://127.0.0.1:1/definitely-unreachable";
        let config = BrowserConfig {
            backend: BrowserBackend::Obscura,
            cdp_url: Some(cdp_url.to_string()),
            obscura_path: obscura_binary.map(|p| p.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let result = BrowserSession::spawn(&config).await;
        let err = match result {
            Ok(_) => panic!("connect to an unreachable URL must fail"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains(cdp_url),
            "error must name the cdp_url that was attempted, proving spawn delegated to \
             connect() rather than local-spawn (which would name obscura_path/the ephemeral \
             port instead), got: {err}"
        );
        assert!(
            err.contains("browser.cdp_url"),
            "error must name browser.cdp_url as the setting, got: {err}"
        );
    }

    #[tokio::test]
    async fn connect_failure_names_the_url() {
        let url = "ws://127.0.0.1:1/unreachable-endpoint";
        let result = BrowserSession::connect(url).await;
        let err = match result {
            Ok(_) => panic!("connect to an unreachable endpoint must fail"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains(url),
            "error must name the URL that failed, got: {err}"
        );
        assert!(
            err.contains("browser.cdp_url"),
            "error must name browser.cdp_url as the setting, got: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_remote_server_closing_does_not_panic_the_pump() {
        let binary = match find_obscura_binary(None) {
            Some(b) => b,
            None => {
                eprintln!(
                    "SKIP a_remote_server_closing_does_not_panic_the_pump: no obscura binary"
                );
                return;
            }
        };
        let config = BrowserConfig::default();
        let (mut child, port) = spawn_obscura_serve(&config, &binary)
            .await
            .expect("helper spawn_obscura_serve must succeed");
        wait_for_obscura_ready(port, Duration::from_secs(10))
            .await
            .expect("helper server must become ready");

        let ws_url = format!("ws://127.0.0.1:{port}");
        let session = BrowserSession::connect(&ws_url)
            .await
            .expect("connect must succeed");

        // Kill the remote server out from under the session — simulates "a remote
        // endpoint that closes mid-session" (RESEARCH.md Open Question 2).
        let _ = child.start_kill();
        let _ = child.wait().await;

        // Phase 53 Plan 03: BrowserSession now implements Drop (T-53-03-03),
        // so it can no longer be destructured by value to pull `handler_task`
        // out on its own (E0509 — cannot move out of a type that implements
        // Drop). Poll `JoinHandle::is_finished()` instead, which only needs
        // `&self`, to prove the pump task exits on its own rather than
        // hanging once the remote endpoint disappears. `handler_task` is a
        // private field but visible here — this `mod tests` is a descendant
        // of the module that defines `BrowserSession`.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if session.handler_task.is_finished() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "pump task must exit within 10s after the remote server closes, not hang"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // close() on this now-dead remote session must still succeed cleanly
        // (T-53-03-01's DropOnly branch — nothing to send, nothing to hang on).
        let _ = session.close().await;
    }

    // =========================================================================
    // Phase 53 (Plan 03 Task 1): per-mode teardown keyed on child ownership
    // =========================================================================

    #[test]
    fn close_on_a_remote_session_does_not_send_browser_close() {
        assert_eq!(
            teardown_action(false, BrowserBackend::Obscura),
            TeardownAction::DropOnly,
            "a session with no owned child driving Obscura (built via connect()) must drop \
             the connection only — never Browser.close, a cross-tenant kill against a server \
             this process does not own"
        );
    }

    #[test]
    fn close_on_a_chromium_session_still_sends_browser_close() {
        assert_eq!(
            teardown_action(false, BrowserBackend::Chromium),
            TeardownAction::SendBrowserClose,
            "today's behaviour (Chromium with no owned child), re-pinned so the refactor \
             cannot silently drop it"
        );
        // Ownership always wins: even a hypothetical future Chromium session
        // that DID record child ownership must terminate the child
        // directly, never reaching for Browser.close.
        assert_eq!(
            teardown_action(true, BrowserBackend::Chromium),
            TeardownAction::TerminateChild
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn close_on_a_local_obscura_session_terminates_its_child() {
        let binary = match find_obscura_binary(None) {
            Some(b) => b,
            None => {
                eprintln!(
                    "SKIP close_on_a_local_obscura_session_terminates_its_child: no obscura \
                     binary"
                );
                return;
            }
        };
        let config = BrowserConfig {
            backend: BrowserBackend::Obscura,
            obscura_path: Some(binary.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let session = BrowserSession::spawn(&config)
            .await
            .expect("BrowserSession::spawn must succeed against a real local obscura binary");
        let pid = session
            .child
            .as_ref()
            .and_then(|c| c.id())
            .expect("a local Obscura spawn must record a child with a live pid");

        session
            .close()
            .await
            .expect("close() on a locally-spawned Obscura session must succeed");

        // Reaping is this test process's own responsibility once close() has
        // terminated the process (we are its real OS parent) — waitpid
        // proves the process is actually gone, not merely "a signal was
        // sent". A process that missed both the SIGTERM and the hard-kill
        // would time this loop out instead of reaping.
        #[cfg(unix)]
        {
            use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
            use nix::unistd::Pid;
            let mut reaped = false;
            for _ in 0..100 {
                match waitpid(Pid::from_raw(pid as i32), Some(WaitPidFlag::WNOHANG)) {
                    Ok(WaitStatus::StillAlive) => {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    _ => {
                        reaped = true;
                        break;
                    }
                }
            }
            assert!(
                reaped,
                "close() must terminate and this process must be able to reap the \
                 locally-spawned obscura child (pid {pid}) promptly"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn close_does_not_hang_when_the_child_ignores_termination() {
        #[cfg(unix)]
        {
            let binary = match find_obscura_binary(None) {
                Some(b) => b,
                None => {
                    eprintln!(
                        "SKIP close_does_not_hang_when_the_child_ignores_termination: no \
                         obscura binary"
                    );
                    return;
                }
            };
            let config = BrowserConfig {
                backend: BrowserBackend::Obscura,
                obscura_path: Some(binary.to_string_lossy().into_owned()),
                ..Default::default()
            };
            let mut session = BrowserSession::spawn(&config)
                .await
                .expect("BrowserSession::spawn must succeed against a real local obscura binary");

            // Swap the real, well-behaved obscura child for one that traps
            // SIGTERM and sleeps well past the grace period, so this test
            // actually exercises the hard-kill escalation rather than the
            // ordinary graceful path the previous test already covers.
            if let Some(mut real_child) = session.child.take() {
                let _ = real_child.start_kill();
                let _ = real_child.wait().await;
            }
            // `echo ready` + a synchronization read below closes the startup
            // race between "shell process exists" and "shell has actually
            // installed the TERM trap" — without it, close()'s SIGTERM can
            // arrive before `trap ''` runs, killing the shell via the
            // still-default SIGTERM disposition and making this test measure
            // shell startup latency instead of the grace-period escalation.
            let mut stubborn = Command::new("/bin/sh")
                .arg("-c")
                .arg("trap '' TERM; echo ready; exec sleep 30")
                .stdout(std::process::Stdio::piped())
                .spawn()
                .expect("spawn stubborn shell");
            {
                use tokio::io::{AsyncBufReadExt, BufReader};
                let stdout = stubborn.stdout.take().expect("piped stdout");
                let mut lines = BufReader::new(stdout).lines();
                let ready = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
                    .await
                    .expect("stubborn shell must announce readiness within 5s")
                    .expect("reading stdout must not error")
                    .expect("stubborn shell stdout must produce a line");
                assert_eq!(
                    ready, "ready",
                    "stubborn shell must confirm its TERM trap is up"
                );
            }
            session.child = Some(stubborn);

            let start = std::time::Instant::now();
            session
                .close()
                .await
                .expect("close() must not error even when the child ignores SIGTERM");
            let elapsed = start.elapsed();

            assert!(
                elapsed < CHILD_TERM_GRACE + Duration::from_secs(5),
                "close() must escalate to a hard kill once the grace period elapses rather \
                 than waiting unboundedly, took {elapsed:?}"
            );
            assert!(
                elapsed >= CHILD_TERM_GRACE,
                "close() must actually wait out the grace period before escalating (a \
                 stubborn child traps SIGTERM), took {elapsed:?}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_a_session_without_close_still_start_kills_its_child() {
        let binary = match find_obscura_binary(None) {
            Some(b) => b,
            None => {
                eprintln!(
                    "SKIP dropping_a_session_without_close_still_start_kills_its_child: no \
                     obscura binary"
                );
                return;
            }
        };
        let config = BrowserConfig {
            backend: BrowserBackend::Obscura,
            obscura_path: Some(binary.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let session = BrowserSession::spawn(&config)
            .await
            .expect("BrowserSession::spawn must succeed against a real local obscura binary");
        let pid = session
            .child
            .as_ref()
            .and_then(|c| c.id())
            .expect("a local Obscura spawn must record a child with a live pid");

        // No close() call — this is the implicit-teardown path (the Option
        // being set to None, or AgentLoop itself dropping).
        drop(session);

        // Drop::drop cannot await, so it can only start_kill() — proven here
        // by reaping the process ourselves (we are its real OS parent)
        // rather than asserting on kill(pid, 0), which would still read a
        // not-yet-reaped zombie as "alive".
        #[cfg(unix)]
        {
            use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
            use nix::unistd::Pid;
            let mut reaped = false;
            for _ in 0..100 {
                match waitpid(Pid::from_raw(pid as i32), Some(WaitPidFlag::WNOHANG)) {
                    Ok(WaitStatus::StillAlive) => {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    _ => {
                        reaped = true;
                        break;
                    }
                }
            }
            assert!(
                reaped,
                "Drop must best-effort start_kill() a locally-spawned child so it does not \
                 outlive the session, pid {pid}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn close_on_a_remote_session_leaves_the_remote_process_running() {
        let binary = match find_obscura_binary(None) {
            Some(b) => b,
            None => {
                eprintln!(
                    "SKIP close_on_a_remote_session_leaves_the_remote_process_running: no \
                     obscura binary"
                );
                return;
            }
        };
        let config = BrowserConfig::default();
        let (mut helper_child, port) = spawn_obscura_serve(&config, &binary)
            .await
            .expect("helper spawn_obscura_serve must succeed");
        wait_for_obscura_ready(port, Duration::from_secs(10))
            .await
            .expect("helper server must become ready");

        let ws_url = format!("ws://127.0.0.1:{port}");
        let session = BrowserSession::connect(&ws_url)
            .await
            .expect("connect must succeed against a server this test itself started");
        assert!(
            session.child.is_none(),
            "connect() must never record child ownership"
        );

        session
            .close()
            .await
            .expect("close() on a remote session must succeed (drop the connection only)");

        // The DropOnly branch never sends anything that could kill the
        // remote process — prove it is still alive and answering its own
        // HTTP control plane after close() returns. (Obscura's own
        // Browser.close handler happens to be a no-op today, so process
        // survival alone cannot by itself distinguish "sent Browser.close"
        // from "sent nothing" — the teardown_action unit tests above and
        // this task's structural <verify> gate are what pin the "never call
        // browser.close() outside the Chromium branch" guarantee; this
        // assertion pins the weaker but still real guarantee that closing a
        // remote session never kills a server this process does not own.)
        wait_for_obscura_ready(port, Duration::from_secs(2))
            .await
            .expect("remote obscura server must still be alive and answering after close()");

        let _ = helper_child.start_kill();
        let _ = helper_child.wait().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn close_on_a_chromium_session_succeeds() {
        if find_chromium_binary(None).is_none() {
            eprintln!("SKIP close_on_a_chromium_session_succeeds: no chromium binary");
            return;
        }
        let config = BrowserConfig::default(); // backend: Chromium (default)
        let session = BrowserSession::spawn(&config)
            .await
            .expect("chromium spawn must succeed");
        assert!(
            session.child.is_none(),
            "Chromium sessions never record child ownership (chromiumoxide/the OS own the \
             process)"
        );
        session
            .close()
            .await
            .expect("close() on a Chromium session must succeed — today's behaviour, unchanged");
    }

    // =========================================================================
    // Phase 53 (Plan 03 Task 2, D-05): refuse a non-render Obscura build
    // =========================================================================

    #[test]
    fn the_probe_requires_both_legs() {
        assert!(
            render_probe_outcome(true, false).is_err(),
            "screenshot succeeding but the hidden element still visible must refuse"
        );
        assert!(
            render_probe_outcome(false, true).is_err(),
            "the hidden element correctly reporting none but screenshot failing must refuse"
        );
        assert!(
            render_probe_outcome(false, false).is_err(),
            "both legs failing must refuse"
        );
        assert!(
            render_probe_outcome(true, true).is_ok(),
            "both legs agreeing must pass"
        );
    }

    #[test]
    fn a_non_render_obscura_build_is_refused_by_name() {
        // The operator's only local obscura binary is render-capable
        // (RESEARCH.md Environment Availability), so this exercises the
        // refusal via the pure two-leg decision rather than a genuine
        // -no-render binary — the true end-to-end refusal is a
        // manual-only verification (53-VALIDATION.md).
        let err = render_probe_outcome(false, false).expect_err("both legs failing must refuse");
        let message = err.to_string();
        assert!(
            message.contains("--features render"),
            "refusal message must name --features render, got: {message}"
        );
        let anyhow_err: anyhow::Error = err.into();
        assert!(
            matches!(
                anyhow_err.downcast_ref::<BrowserStartError>(),
                Some(BrowserStartError::RenderUnsupported { .. })
            ),
            "must downcast to BrowserStartError::RenderUnsupported for Task 3's diagnose()"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_render_capable_backend_passes_the_probe() {
        let binary = match find_obscura_binary(None) {
            Some(b) => b,
            None => {
                eprintln!("SKIP a_render_capable_backend_passes_the_probe: no obscura binary");
                return;
            }
        };
        let config = BrowserConfig {
            backend: BrowserBackend::Obscura,
            obscura_path: Some(binary.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let session = BrowserSession::spawn(&config).await.expect(
            "spawn must succeed against a render-capable obscura binary (the probe must not \
             refuse a genuinely working build)",
        );
        let _ = session.close().await;
    }

    #[test]
    fn the_probe_does_not_run_on_the_chromium_branch() {
        let source = include_str!("browser_session.rs");
        let needle = format!("{}{}", "pub async fn ", "spawn(");
        let start = source
            .find(&needle)
            .expect("spawn fn must exist in this file");
        let after_start = &source[start..];
        let chromium_marker = "// D-02: cdp_url is inert under backend: chromium";
        let chromium_start = after_start
            .find(chromium_marker)
            .expect("D-02 comment marks the start of the Chromium path in spawn()");
        let end_marker = "\n    }\n";
        let end = after_start
            .find(end_marker)
            .expect("spawn fn must have a closing brace at 4-space indent")
            + end_marker.len();
        let chromium_body = &after_start[chromium_start..end];
        assert!(
            !chromium_body.contains("probe_render_support"),
            "the Chromium path in spawn() must never call probe_render_support (D-05 scope)"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_session_returned_on_success_is_on_about_blank() {
        let binary = match find_obscura_binary(None) {
            Some(b) => b,
            None => {
                eprintln!(
                    "SKIP the_session_returned_on_success_is_on_about_blank: no obscura binary"
                );
                return;
            }
        };
        let config = BrowserConfig {
            backend: BrowserBackend::Obscura,
            obscura_path: Some(binary.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let session = BrowserSession::spawn(&config)
            .await
            .expect("spawn must succeed against a render-capable obscura binary");
        let url = session.page.url().await.expect("page.url() must succeed");
        assert_eq!(
            url.as_deref(),
            Some("about:blank"),
            "spawn's contract (browser_session.rs:45-46) is a fresh about:blank page; the \
             render probe must restore it after navigating away to run its own fixture"
        );
        let _ = session.close().await;
    }
}
