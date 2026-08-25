mod app;
mod components;
mod fonts;
// Phase 36.3.7.11 Plan 02: client-side kanban utilities (transition
// validator + drag-blocked hint copy). The server-side `kanban_api`
// imports `kanban::transitions::is_drag_allowed` for D-14 defense-in-depth.
mod kanban;
mod mocks;
mod platform;
mod protocol;
mod server;
mod state;
mod ui_prefs;

use app::App;

/// D-10 fail-closed startup invariant: a non-loopback bind with no
/// configured operator password hash refuses to start. Pure and
/// side-effect-free (no logging, no panicking, no env reads) so the
/// invariant is provable by unit test without spawning a process — see
/// `mod tests` below. Today's shell-script gate (`scripts/deploy/web-run.sh`,
/// `web-dev.sh`) only advises; this is the real guarantee, reachable no
/// matter how the binary is launched (directly, via systemd, etc.).
#[cfg(feature = "server")]
fn bind_guard_allows(ip: std::net::IpAddr, auth_enabled: bool) -> bool {
    ip.is_loopback() || auth_enabled
}

/// Quick task 260818-t3y: compose the operator-facing refusal message for
/// the provider-key startup guard. Pure (no logging, no panicking) so the
/// message contents are provable by unit test — see `mod tests` below.
///
/// Names, in order: the main provider, its resolved endpoint `base_url`, the
/// exact env var to set (falling back to a `providers.<provider>.api_key_env`
/// instruction when no name is resolvable), the `~/.ironhermes/.env` path the
/// value belongs in, and the container remedy. Never includes the key VALUE.
#[cfg(feature = "server")]
fn provider_key_guard_refusal_message(
    provider: &str,
    base_url: &str,
    config: &ironhermes_core::config::Config,
) -> String {
    let env_path = ironhermes_core::config::Config::env_path();
    match ironhermes_core::provider::main_provider_key_env_name(config) {
        Some(env_name) => format!(
            "refusing to start: main provider '{provider}' (endpoint {base_url}) has no \
             resolvable API key. Set {env_name} — add it to {} (loaded at startup), or pass \
             it as a container env var (`-e {env_name}=...` on `podman run` / `docker run`).",
            env_path.display()
        ),
        None => format!(
            "refusing to start: main provider '{provider}' (endpoint {base_url}) has no \
             resolvable API key, and no `providers.{provider}.api_key_env` is configured to \
             name one. Set `providers.{provider}.api_key_env` in config.yaml to the name of \
             the env var holding your key, then set that variable in {} (loaded at startup) \
             or pass it as a container env var.",
            env_path.display()
        ),
    }
}

#[cfg(feature = "server")]
#[tokio::main]
async fn main() {
    use dioxus::prelude::*;
    use tower_http::trace::{DefaultOnRequest, DefaultOnResponse, TraceLayer};
    use tracing::Level;

    // Install the global tracing subscriber: stdout console + daily-rolling
    // web.log (app/agent) + daily-rolling web-access.log (HTTP access). Both
    // WorkerGuards MUST be held for the lifetime of axum::serve(...).await —
    // dropping them shuts down the non-blocking writer threads and any
    // buffered log lines are lost. The underscore prefix silences the
    // unused-variable lint while extending the lifetime to the end of main().
    let (_web_log_guard, _access_log_guard) = server::logging::install_web_logger_subscriber();

    // Load ~/.ironhermes/.env so OPENROUTER_API_KEY / ANTHROPIC_API_KEY etc.
    // are available to the embedded agent — mirrors what the CLI does in main.rs.
    let env_path = ironhermes_core::config::Config::env_path();
    if env_path.exists() {
        dotenvy::from_path(&env_path).ok();
    }

    // Initialize shared state at server startup.
    let app_state = server::state::AppState::init()
        .await
        .expect("Failed to initialize AppState");
    server::state::install_global_app_state(app_state.clone())
        .expect("Failed to install global AppState");

    let address = dioxus::cli_config::fullstack_address_or_localhost();

    // Explicit Level::INFO on both request and response callbacks is mandatory:
    // tower_http::trace::DefaultOnRequest / DefaultOnResponse default to
    // Level::DEBUG, which the locked access-log filter `tower_http::trace=info`
    // (server/logging.rs) would silently drop — leaving web-access.log empty.
    // See RESEARCH.md Q2 / Pitfall 1.
    let trace_layer = TraceLayer::new_for_http()
        .on_request(DefaultOnRequest::new().level(Level::INFO))
        .on_response(DefaultOnResponse::new().level(Level::INFO));

    // Phase 47.3 Plan 01 (D-06/D-07): resolve the operator auth config
    // (config.yaml > IRONHERMES_WEB_PASSWORD_HASH env > vault fallback — a
    // sealed/corrupt vault is a hard startup error, never a silent
    // auth-disabled fall-through) and build the shared AuthState. A
    // malformed `web_ui.auth.password_hash` PHC string is also a startup
    // error here (AuthState::new), not a per-login 500.
    let auth_config = server::auth::auth_config_from(&app_state.config)
        .await
        .expect("failed to resolve web_ui.auth config (sealed/corrupt vault, or malformed hash)");
    let auth_state = server::auth::AuthState::new(auth_config)
        .expect("web_ui.auth.password_hash is not a valid argon2id PHC string");

    // Phase 47.3 Plan 02 (D-10): the fail-closed bind guard. Must run BEFORE
    // TcpListener::bind so a refused configuration never opens a listening
    // socket — running the binary directly, or via a systemd unit that
    // never calls scripts/deploy/web-run.sh, must still refuse a
    // non-loopback bind with no configured password hash (the shell gate
    // becomes advisory, this is the real guarantee).
    if !bind_guard_allows(address.ip(), auth_state.enabled()) {
        let msg = format!(
            "refusing to bind {address}: this is a non-loopback address and no \
             web_ui.auth.password_hash is configured. Set one via \
             `ironhermes web set-password` (paste the printed hash into \
             web_ui.auth.password_hash in config.yaml), or bind to a loopback \
             address (127.0.0.1 / ::1) instead."
        );
        tracing::error!(target: "iron_hermes_ui", "{msg}");
        panic!("{msg}");
    }

    // Quick task 260818-t3y: the provider-key startup guard. Runs AFTER the
    // bind guard above so a config that fails both guards still reports the
    // bind problem first, preserving existing behavior for deployments that
    // already pass this guard today. Reuses `app_state.resolver` — already
    // built by `ProviderResolver::build()` AND vault-fallback-applied inside
    // `AppState::init()` — rather than re-running `ProviderResolver::build`
    // here, which would silently drop any key resolved only via the vault
    // and produce a false-positive refusal.
    //
    // Deliberately NOT enforced in `docker/entrypoint.sh` (the `ironhermes`
    // CLI): `--help`, `version`, `doctor`, `web set-password`, and
    // `provider list` must keep working with no key configured so operators
    // can introspect a broken config. This guard is a property of the
    // always-agent-serving web binary, which is the only thing
    // `docker/web-entrypoint.sh` execs — do not "fix" that asymmetry.
    let main_provider = app_state.config.model.provider.clone();
    match app_state.resolver.resolve(&main_provider) {
        Some(endpoint) => {
            if !ironhermes_core::provider::provider_key_guard_allows(
                endpoint.api_key.as_deref(),
                &endpoint.base_url,
            ) {
                let msg = provider_key_guard_refusal_message(
                    &main_provider,
                    &endpoint.base_url,
                    &app_state.config,
                );
                tracing::error!(target: "iron_hermes_ui", "{msg}");
                panic!("{msg}");
            }
        }
        None => {
            let msg = format!(
                "refusing to start: main provider '{main_provider}' (model.provider in \
                 config.yaml) is not a known provider — define it under `providers:` in \
                 config.yaml, or fix `model.provider` to name one that is."
            );
            tracing::error!(target: "iron_hermes_ui", "{msg}");
            panic!("{msg}");
        }
    }

    let auth_routes = axum::Router::new()
        .route("/auth/login", axum::routing::post(server::auth::login))
        .route("/auth/session", axum::routing::get(server::auth::session_probe))
        .route("/auth/logout", axum::routing::post(server::auth::logout))
        // /auth/* carries its own tight body limit — the global 16MB limit
        // below is sized for attachments, not a ~1KB login POST.
        .layer(axum::extract::DefaultBodyLimit::max(4096))
        .with_state(auth_state.clone());

    // Phase 47.3 Plan 01 (D-07 / RESEARCH.md Pattern 1): build the SAME three
    // calls `serve_dioxus_application` performs internally
    // (register_server_functions -> serve_static_assets -> fallback+with_state)
    // ourselves, with `.route("/", get(root_handler))` inserted BEFORE the
    // final `.with_state(...)`. This is required (not stylistic): once state
    // is erased to `Router<()>`, a route added afterward cannot use a
    // `State<FullstackState>` extractor. Registering root_handler on the same
    // typed `Router<FullstackState>` keeps that extractor valid while the
    // exact-path route still wins over `.fallback()` (Assumption A1).
    let fullstack_state = dioxus::server::FullstackState::new(ServeConfig::new(), App);
    let router = axum::Router::new()
        .register_server_functions()
        .serve_static_assets()
        .route("/", axum::routing::get(server::login_page::root_handler))
        .fallback(axum::routing::get(dioxus::server::render_handler))
        .with_state(fullstack_state)
        // Phase 46.6 gap-closure: raw axum route for artifact serving. The
        // `#[get]` server-fn codec serialized the `Vec<u8>` return as a JSON
        // number array (`[60,104,...]`), breaking the iframe viewer + direct
        // nav. This handler serves raw `text/html` with the unconditional D-02
        // CSP and real 400/404 status codes. axum 0.8 path syntax: `{id}`.
        .route(
            "/artifacts/{id}",
            axum::routing::get(server::artifact_route::serve_artifact),
        )
        // Phase 46.7 Plan 05 (D-06): raw axum route for session-attachment
        // serving (sent-bubble image thumbnails + non-image download chips).
        // Same JSON-number-array codec pitfall as /artifacts/{id} above.
        .route(
            "/chat-attachments/{session_id}/{id}",
            axum::routing::get(server::chat_attachment_route::serve_chat_attachment),
        )
        // Phase 50.1 Plan 04 (D-11/D-12): raw axum route for bot avatar
        // serving (roster card / drawer / picker preview <img src>). Same
        // JSON-number-array codec pitfall as the two routes above.
        .route(
            "/bot-avatars/{name}/{image_id}",
            axum::routing::get(server::bot_avatar_route::serve_bot_avatar),
        )
        // Phase 48.2 Plan 09 (D-03/T-48.2-09-01): raw axum route for the MCP
        // OAuth web callback (path must match
        // `mcp_admin_api::MCP_OAUTH_CALLBACK_PATH` and `auth::is_public`'s
        // matching arm exactly). An authorization server redirects a
        // *browser* here with an ordinary GET that must return HTML, which
        // the server-fn codec cannot serve — same rationale as the three raw
        // routes above. Public (see `auth::is_public` and this route
        // module's own doc comment for why); mounted here, BEFORE the auth
        // layer below, exactly like every other route in this block.
        .route(
            "/oauth/mcp/callback",
            axum::routing::get(server::mcp_oauth_callback_route::serve_mcp_oauth_callback),
        )
        // Phase 47.3 Plan 01: /auth/login, /auth/session, /auth/logout —
        // raw axum routes (same server-fn JSON-codec pitfall precedent).
        .merge(auth_routes)
        .layer(axum::Extension(app_state))
        // Phase 47.3 Plan 01: exposes AuthState to root_handler via
        // Extension (root_handler's own State<FullstackState> extractor is
        // satisfied by the router's own state above, so AuthState rides in
        // as an Extension instead).
        .layer(axum::Extension(auth_state.clone()))
        // Phase 46.4 UAT gap-fix: raise the request-body limit above axum's
        // 2MB default. Attachments post base64-in-JSON, so the 10MB decoded
        // cap (kanban_api::MAX_ATTACHMENT_BYTES) inflates to ~13.3MB on the
        // wire. Without this, any file over ~1.5MB was rejected at the HTTP
        // layer (413) BEFORE the 10MB cap could run, surfacing as a generic
        // "upload failed". 16MB gives headroom for the base64 body + JSON envelope.
        .layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024))
        // Phase 47.3 Plan 01 (D-07): the deny-by-default auth boundary.
        // MUST be registered after every route/merge above (axum layers wrap
        // only previously-registered routes) so it wraps server fns, the WS
        // upgrade, artifacts, attachments, and the two raw routes above.
        .layer(axum::middleware::from_fn_with_state(
            auth_state,
            server::auth::require_auth,
        ))
        // 47.3 UAT 1i follow-up: compress response bodies. Registered AFTER the
        // auth layer so it wraps it — the pre-auth login document is the largest
        // single response we serve (~206KB of orbiter markup, ~13KB gzipped) and
        // is returned by the auth layer itself, so a layer inside it would never
        // see that body. Shared with `test_router` via one definition.
        .layer(server::login_page::compression_layer())
        .layer(trace_layer)
        .into_make_service_with_connect_info::<std::net::SocketAddr>();

    let listener = tokio::net::TcpListener::bind(address).await.unwrap();

    // Deterministic startup marker — fires before the server starts accepting
    // traffic so web.log has at least one INFO line on every boot, even when
    // the agent loop never runs (e.g. brief UAT lifecycle). Without this,
    // a short-lived process may only emit lower-priority events that match the
    // filter, leaving web.log empty by happenstance.
    tracing::info!(target: "iron_hermes_ui", "server bound to {address}");

    // Graceful shutdown is load-bearing for file logging: SIGTERM / Ctrl-C
    // signal axum to complete in-flight requests and return, which lets
    // main() unwind and drop both _web_log_guard and _access_log_guard. The
    // guard Drop impls synchronously join the non-blocking writer threads,
    // flushing any buffered log lines to disk. Without with_graceful_shutdown,
    // the runtime is hard-killed before main() returns — guards never drop —
    // and the last batch of buffered tracing events is silently lost.
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();
}

// Resolves when the process receives SIGTERM (POSIX container shutdown) or
// SIGINT (terminal Ctrl-C). Wired into axum's with_graceful_shutdown so the
// WorkerGuards held in main() get a chance to flush before process exit.
#[cfg(feature = "server")]
async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        let _ = signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(not(feature = "server"))]
fn main() {
    dioxus::launch(App);
}

// D-10: the six `bind_guard_allows` combinations named in
// `47.3-02-PLAN.md`'s Task 1 behavior block. Gated the same way the
// function itself is (`server`-only) so `cargo test -p iron_hermes_ui
// --all-features bind_guard` reaches this module.
#[cfg(all(test, feature = "server"))]
mod tests {
    use super::bind_guard_allows;
    use std::net::IpAddr;

    #[test]
    fn bind_guard_loopback_v4_no_auth_allowed() {
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(bind_guard_allows(ip, false));
    }

    #[test]
    fn bind_guard_loopback_v4_with_auth_allowed() {
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(bind_guard_allows(ip, true));
    }

    #[test]
    fn bind_guard_loopback_v6_no_auth_allowed() {
        let ip: IpAddr = "::1".parse().unwrap();
        assert!(bind_guard_allows(ip, false));
    }

    #[test]
    fn bind_guard_public_bind_with_auth_allowed() {
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(
            bind_guard_allows(ip, true),
            "a public bind WITH a configured hash is exactly the posture this phase enables"
        );
    }

    #[test]
    fn bind_guard_public_bind_without_auth_refused() {
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(!bind_guard_allows(ip, false), "the one refused combination");
    }

    #[test]
    fn bind_guard_wildcard_bind_without_auth_refused() {
        let ip: IpAddr = "0.0.0.0".parse().unwrap();
        assert!(
            !bind_guard_allows(ip, false),
            "wildcard bind is not loopback"
        );
    }

    // Quick task 260818-t3y: behavior Test 1 — the composed refusal message
    // reaches this module without spawning a process and names the env var
    // the operator must set. Reachable via `cargo test -p iron_hermes_ui
    // --features server provider_key` (mirrors the bind_guard invocation
    // pattern above).
    #[test]
    fn provider_key_guard_refusal_message_names_env_var() {
        let config = ironhermes_core::config::Config::default();
        let msg = super::provider_key_guard_refusal_message(
            "openrouter",
            "https://openrouter.ai/api/v1",
            &config,
        );
        assert!(
            msg.contains("OPENROUTER_API_KEY"),
            "refusal message must name the exact env var to set, got: {msg}"
        );
        assert!(
            msg.contains("openrouter"),
            "refusal message must name the provider, got: {msg}"
        );
        assert!(
            msg.contains("https://openrouter.ai/api/v1"),
            "refusal message must name the endpoint base_url, got: {msg}"
        );
    }

    #[test]
    fn provider_key_guard_refusal_message_unknown_provider_names_config_key() {
        let mut config = ironhermes_core::config::Config::default();
        config.model.provider = "totally_unknown".to_string();
        let msg = super::provider_key_guard_refusal_message(
            "totally_unknown",
            "https://example.com/v1",
            &config,
        );
        assert!(
            msg.contains("providers.totally_unknown.api_key_env"),
            "with no resolvable env var name, the message must point at the config key \
             instead, got: {msg}"
        );
    }
}
