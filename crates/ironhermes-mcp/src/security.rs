use ironhermes_core::audit::{AuditConfig, AuditLog};
use ironhermes_core::content_screen::{
    ContentKind, ContentScreen, ContentScreenRequest, ScreenVerdict,
};
use ironhermes_core::context_scanner::{SECURITY_VERDICT_KIND, SIDECAR_UNAVAILABLE_KIND};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

/// D-19: Safe environment key allowlist for stdio subprocess.
/// Only these keys (plus XDG_* prefix) are passed to MCP server child processes.
/// User-specified `env` values from config are added on top.
const SAFE_ENV_KEYS: &[&str] = &[
    "PATH", "HOME", "USER", "LANG", "LC_ALL", "TERM", "SHELL", "TMPDIR",
];

/// Build a safe environment for a stdio MCP server subprocess.
///
/// Returns a HashMap containing:
/// 1. All allowlisted keys from the host environment (`SAFE_ENV_KEYS` + `XDG_*`).
/// 2. All user-specified `env` vars from the server config (may override allowlist).
///
/// This matches hermes-agent's `_build_safe_env()` (D-19).
pub fn build_safe_env(user_env: &HashMap<String, String>) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std::env::vars()
        .filter(|(k, _)| SAFE_ENV_KEYS.contains(&k.as_str()) || k.starts_with("XDG_"))
        .collect();
    env.extend(user_env.iter().map(|(k, v)| (k.clone(), v.clone())));
    env
}

/// D-20: Credential pattern — matches sensitive tokens/keys that must be redacted.
///
/// Patterns ported from hermes-agent's `_CREDENTIAL_PATTERN`:
/// - GitHub tokens: `ghp_*`
/// - OpenAI/Anthropic API keys: `sk-*`
/// - Bearer tokens: `Bearer <value>`
/// - Generic credential assignments: `token=`, `key=`, `API_KEY=`, `password=`, `secret=`
pub static CREDENTIAL_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    // Note: use regular string literals (not raw) for the character classes that
    // contain backslash escapes (\s, \S) and quote characters.
    Regex::new(concat!(
        r"(?i)(?:",
        r"ghp_[A-Za-z0-9_]{1,255}",
        r"|sk-[A-Za-z0-9_]{1,255}",
        "|Bearer\\s+\\S+",
        "|token=[^\\s&,;\"']{1,255}",
        "|key=[^\\s&,;\"']{1,255}",
        "|API_KEY=[^\\s&,;\"']{1,255}",
        "|password=[^\\s&,;\"']{1,255}",
        "|secret=[^\\s&,;\"']{1,255}",
        r")",
    ))
    .unwrap()
});

/// B-4/D-01: Built-in baseline issuer host suffixes for PRM (Protected Resource
/// Metadata) discovery.
///
/// Always included when a server declares no per-server `allowed_issuer` pin —
/// the existing 4 Cloudflare servers keep working with zero new config (CFL-02
/// no-regression). Config-driven extension is now live via
/// `Config.mcp_oauth.issuer_allowlist` (global additive) and
/// `McpServerConfig.allowed_issuer` (per-server pin, authoritative) — see
/// [`resolve_allowed_issuers`].
pub const BASELINE_ISSUER_ALLOWLIST: &[&str] = &["cloudflare.com", "dash.cloudflare.com"];

/// D-01: Resolve the effective allowed-issuer set for one MCP server.
///
/// Layered model:
/// - **Primary (per-server pin):** when `per_server_pin` is `Some` with a
///   non-empty (post-trim) value, it is **authoritative** — the returned set is
///   exactly `[pin]` and `global_additive` is NOT consulted. A pin is always at
///   least as strict as the global matcher by construction (a single-domain
///   allowlist is ⊆ any multi-domain one).
/// - **Fallback (global additive list):** when no pin is present (or the pin is
///   empty/whitespace-only — V5 fail-safe: treated as absent, never as an
///   empty/deny-all set), the returned set is `BASELINE_ISSUER_ALLOWLIST`
///   unioned with `global_additive`.
pub fn resolve_allowed_issuers(
    per_server_pin: Option<&str>,
    global_additive: &[String],
) -> Vec<String> {
    match per_server_pin.map(str::trim).filter(|p| !p.is_empty()) {
        Some(pin) => vec![pin.to_string()],
        None => BASELINE_ISSUER_ALLOWLIST
            .iter()
            .map(|s| s.to_string())
            .chain(global_additive.iter().cloned())
            .collect(),
    }
}

/// D-02 seam: returns `true` when `issuer_url`'s host matches the built-in
/// baseline allowlist (`BASELINE_ISSUER_ALLOWLIST`), using the same
/// dot-anchored suffix match as [`validate_prm_issuer`]. Returns `false` on a
/// parse failure or when the host is not a baseline entry — including hosts
/// only reachable via a per-server pin or the global additive list. Consumed
/// by plan 02's new-issuer detection (D-02).
pub fn is_baseline_issuer(issuer_url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(issuer_url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    BASELINE_ISSUER_ALLOWLIST.iter().any(|domain| {
        let domain = domain.to_ascii_lowercase();
        host == domain || host.ends_with(&format!(".{domain}"))
    })
}

/// B-4: Validate a PRM issuer URL before following server-advertised auth metadata.
///
/// Rejects:
/// - Malformed URLs (must parse as an absolute URL with a host)
/// - Non-HTTPS URLs (HTTP is never allowed for auth metadata)
/// - Issuers whose **parsed host** does not match any entry in `allowed`
///
/// Matching is performed against the parsed host with anchored comparison —
/// exact equality or a dot-anchored suffix (`host == d || host.ends_with(".{d}")`).
/// A naive `issuer_url.contains(domain)` check would be an allowlist bypass: hosts
/// like `attacker-cloudflare.com` or `cloudflare.com.evil.com`, or a URL that merely
/// mentions an allowlisted domain in its path/query/userinfo, would all slip through.
///
/// D-01: `allowed` is the resolved set for this connect attempt (see
/// [`resolve_allowed_issuers`]) — callers must resolve the set (per-server pin,
/// else baseline ∪ global) BEFORE calling this function; the matcher itself is
/// unchanged from its pre-46.1 const-only form.
pub fn validate_prm_issuer(issuer_url: &str, allowed: &[String]) -> anyhow::Result<()> {
    let parsed = url::Url::parse(issuer_url).map_err(|e| {
        anyhow::anyhow!("PRM issuer rejected: invalid URL '{issuer_url}': {e} (B-4)")
    })?;
    if parsed.scheme() != "https" {
        anyhow::bail!("PRM issuer rejected: non-HTTPS URL '{issuer_url}' (B-4)");
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("PRM issuer rejected: no host in '{issuer_url}' (B-4)"))?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let allowed_ok = allowed.iter().any(|domain| {
        let domain = domain.to_ascii_lowercase();
        host == domain || host.ends_with(&format!(".{domain}"))
    });
    if !allowed_ok {
        anyhow::bail!("PRM issuer rejected: host '{host}' not in allowlist (B-4)");
    }
    Ok(())
}

/// Strip credential patterns from error text before returning to the LLM (D-20).
///
/// Replaces all credential matches with `[REDACTED]`.
/// Matches hermes-agent's `_sanitize_error()`.
///
/// 54.8-16 (T-54.8-49): also applies [`crate::oauth_redact::redact_oauth_params`],
/// so an OAuth `code`/`state` (or token parameter) quoted in an exchange error —
/// which `CREDENTIAL_PATTERN` has no arm for — never survives sanitization.
pub fn sanitize_error(text: &str) -> String {
    let credentials_masked = CREDENTIAL_PATTERN.replace_all(text, "[REDACTED]");
    crate::oauth_redact::redact_oauth_params(&credentials_masked).into_owned()
}

/// Fixed markers a `run_server_task` failure reason carries when the failure
/// is auth-caused — a stored OAuth credential exists but cannot be used
/// (refresh failed, scope insufficient, or a bounded 401-retry was
/// exhausted) — as opposed to a genuine transport/spawn failure unrelated to
/// authorization.
///
/// Each marker is the fixed, sanitize-error-surviving prefix or substring a
/// call site in this crate already produces for exactly one such case:
/// - `"Get access token: "` — `transport::connect_http_oauth`'s hot path
///   (cached token present, `get_access_token()` failed: the refresh call
///   itself errored, e.g. the authorization server returned `invalid_grant`).
/// - `"OAuth insufficient_scope"` / `"OAuth token refresh + 1-retry both
///   returned 401"` — the two `McpOAuthError` variants' `Display` text
///   (`server_task.rs`), both explicitly documented as requiring
///   re-authorization.
/// - `"OAuth 401 at discovery"` — the transient 401-at-discovery message
///   that triggers `server_task`'s one-shot forced refresh; treated as
///   auth-caused because its only two outcomes are a successful reconnect
///   (this reason is superseded) or one of the two `McpOAuthError` markers
///   above.
const OAUTH_REAUTH_MARKERS: &[&str] = &[
    "Get access token: ",
    "OAuth insufficient_scope",
    "OAuth token refresh + 1-retry both returned 401",
    "OAuth 401 at discovery",
];

/// `true` when a `run_server_task`/`ServerTaskResult` failure reason
/// indicates the stored OAuth credential is unusable and the operator must
/// re-authorize, rather than a genuine spawn/transport failure unrelated to
/// authorization.
///
/// Pure substring match against [`OAUTH_REAUTH_MARKERS`] — no I/O, no
/// credential-bearing input. Callers (the web admin API's live status
/// classifier) use this to decide whether a not-connected OAuth server with
/// a present-but-dead token should present as needing (re)authorization
/// instead of as a generic failure.
pub fn is_oauth_reauthorization_required(reason: &str) -> bool {
    OAUTH_REAUTH_MARKERS
        .iter()
        .any(|marker| reason.contains(marker))
}

/// Maximum allowed length, in bytes, for a `web_redirect_base_url` input to
/// [`validate_web_redirect_base`]. Prevents pathological inputs from ever
/// reaching `url::Url::parse` or the error path.
const WEB_REDIRECT_BASE_MAX_LEN: usize = 255;

/// Phase 48.2 Plan 08 (D-13/T-48.2-08-02): validate an operator- or
/// caller-supplied absolute origin intended for `Config.mcp_oauth.web_redirect_base_url`
/// or an equivalent caller-supplied redirect base.
///
/// Pure (no I/O) so it is trivially unit-testable and safely callable from
/// `iron_hermes_ui`. Parses `base` with [`url::Url`] and rejects:
/// - a scheme other than `http` or `https`
/// - an absent or empty host
/// - a non-empty username, or any password (userinfo)
/// - a path other than empty or a single slash
/// - any query string
/// - any fragment
/// - an input longer than [`WEB_REDIRECT_BASE_MAX_LEN`] bytes
///
/// Every rejection returns its own FIXED message that never echoes the input
/// value — the same "never echo an input field" discipline `sanitize_error`
/// enforces elsewhere in this module.
///
/// On success, returns the normalized origin via `Url::origin().ascii_serialization()`
/// (`scheme://host[:port]`, default ports elided, no trailing slash).
pub fn validate_web_redirect_base(base: &str) -> Result<String, String> {
    if base.len() > WEB_REDIRECT_BASE_MAX_LEN {
        return Err(format!(
            "web redirect base rejected: input exceeds {WEB_REDIRECT_BASE_MAX_LEN} bytes"
        ));
    }

    let parsed = url::Url::parse(base)
        .map_err(|_| "web redirect base rejected: not a valid URL".to_string())?;

    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("web redirect base rejected: scheme must be http or https".to_string());
    }

    match parsed.host_str() {
        Some(h) if !h.is_empty() => {}
        _ => return Err("web redirect base rejected: URL must have a non-empty host".to_string()),
    }

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("web redirect base rejected: URL must not contain userinfo".to_string());
    }

    if !parsed.path().is_empty() && parsed.path() != "/" {
        return Err(
            "web redirect base rejected: URL must not contain a path beyond '/'".to_string(),
        );
    }

    if parsed.query().is_some() {
        return Err("web redirect base rejected: URL must not contain a query string".to_string());
    }

    if parsed.fragment().is_some() {
        return Err("web redirect base rejected: URL must not contain a fragment".to_string());
    }

    Ok(parsed.origin().ascii_serialization())
}

// ---------------------------------------------------------------------------
// Phase 57 plan 08 (D-04c / D-04d / D-12): sidecar screening of MCP servers.
// ---------------------------------------------------------------------------

/// `AuditEntry::tool` of MCP server-configuration screening rows.
pub const MCP_CONFIG_TOOL: &str = "mcp_config";
/// `AuditEntry::tool` of MCP endpoint-URL screening rows.
pub const MCP_ENDPOINT_TOOL: &str = "mcp_endpoint";
/// The file name the sidecar sees for a rendered server config. tirith's MCP rules
/// key on this name (research OQ6, probed positive in `context_and_mcp_probe.rs`).
pub const MCP_CONFIG_SCREEN_NAME: &str = "mcp.json";

/// The audit log rows are appended to. Tests redirect it to a private temp file.
fn audit_log() -> AuditLog {
    #[cfg(test)]
    if let Some(path) = test_support::audit_path_override() {
        return AuditLog::with_path(path, AuditConfig::default());
    }
    AuditLog::load(AuditConfig::default())
}

/// Append one screening row. Rows carry rule ids, the server name and the level
/// only: never a URL with credentials, never config text. A failed write is logged,
/// never fatal.
fn write_screen_row(
    tool: &str,
    kind: &str,
    server: Option<&str>,
    reason: String,
    decision: &str,
    level: u8,
    rules: &[String],
) {
    let log = audit_log();
    let args = serde_json::json!({ "level": level, "rules": rules });
    let entry = log.make_entry(
        kind,
        tool,
        server.map(str::to_string),
        "",
        "mcp",
        "",
        reason,
        decision,
        "policy",
        &args,
    );
    if let Err(e) = log.append_sync(&entry) {
        tracing::warn!("could not write {kind} audit row for {tool}: {e}");
    }
}

/// `scheme://host[:port]` of `raw`, for error text and logs. Userinfo, path, query
/// and fragment are dropped: any of them can carry a credential.
pub fn redact_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(u) => u.origin().ascii_serialization(),
        Err(_) => "<unparseable url>".to_string(),
    }
}

/// Why a screening did not come back clean: rule ids, else the unavailable reason.
fn screen_reason(rules: &[String], unavailable: Option<&str>) -> String {
    if let Some(reason) = unavailable {
        format!("security sidecar unavailable: {reason}")
    } else if rules.is_empty() {
        "no rule ids reported".to_string()
    } else {
        format!("rules: {}", rules.join(", "))
    }
}

/// Screen one rendered server config (D-04c, research OQ6 POSITIVE) and decide
/// whether the server may start. Warn-class (D-12): a sidecar that cannot answer
/// is Warn at DEFCON 4-5 (server starts, with a row saying why) and Block at 1-3
/// (server skipped). Returns `true` when the server may start.
pub fn screen_mcp_config(
    screen: &dyn ContentScreen,
    server: &str,
    rendered_json: &str,
    level: ironhermes_core::config::DefconLevel,
) -> bool {
    let outcome = screen.screen(&ContentScreenRequest {
        kind: ContentKind::File,
        text: rendered_json,
        name: Some(MCP_CONFIG_SCREEN_NAME),
        level,
        warn_class: true,
    });
    let unavailable = outcome.unavailable.as_deref();
    let kind = if unavailable.is_some() {
        SIDECAR_UNAVAILABLE_KIND
    } else {
        SECURITY_VERDICT_KIND
    };
    let reason = format!("{server}: {}", screen_reason(&outcome.rules, unavailable));
    match outcome.verdict {
        ScreenVerdict::Block => {
            tracing::error!(
                server = %server,
                rules = ?outcome.rules,
                unavailable = ?unavailable,
                "MCP server configuration blocked by the security screen; not starting it"
            );
            write_screen_row(
                MCP_CONFIG_TOOL,
                kind,
                Some(server),
                reason,
                "blocked",
                level.level(),
                &outcome.rules,
            );
            false
        }
        ScreenVerdict::Warn => {
            tracing::warn!(
                server = %server,
                rules = ?outcome.rules,
                unavailable = ?unavailable,
                "MCP server configuration flagged by the security screen; starting it"
            );
            write_screen_row(
                MCP_CONFIG_TOOL,
                kind,
                Some(server),
                reason,
                "warned",
                level.level(),
                &outcome.rules,
            );
            true
        }
        ScreenVerdict::Allow => {
            if unavailable.is_some() {
                write_screen_row(
                    MCP_CONFIG_TOOL,
                    kind,
                    Some(server),
                    reason,
                    "allow",
                    level.level(),
                    &outcome.rules,
                );
            }
            true
        }
    }
}

/// Screen one resolved MCP endpoint URL (D-04d) as the exec command
/// `curl '<url>'`, warn-class (D-12), and refuse the connect on a Block.
///
/// This is in addition to, never instead of, any SSRF guard: tirith reports
/// metadata endpoints, raw-IP plain-http and punycode hosts, but nothing for
/// `localhost` or private ranges.
///
/// - Block: `Err` naming the rule ids (or the unavailable reason).
/// - Warn at DEFCON 4-5 (bypass levels): `Ok`, with a `security_verdict` row.
/// - Warn at DEFCON 1-3: `Err`. An MCP connect has no approval prompt, so the
///   conservative reading of D-07 for a non-interactive connect is to refuse.
/// - Unavailable sidecar: the outcome verdict already encodes D-12 (Warn at 4-5,
///   Block at 1-3); the reason is carried in the error or in a
///   `sidecar_unavailable` row.
///
/// Errors and rows name only `scheme://host[:port]`: userinfo, path and query
/// (where credentials live) are never echoed.
pub fn screen_endpoint_url_with(
    screen: &dyn ContentScreen,
    url: &str,
    level: ironhermes_core::config::DefconLevel,
) -> anyhow::Result<()> {
    let quoted = url.replace('\'', r"'\''");
    let text = format!("curl '{quoted}'");
    let outcome = screen.screen(&ContentScreenRequest {
        kind: ContentKind::Exec,
        text: &text,
        name: None,
        level,
        warn_class: true,
    });
    let shown = redact_url(url);
    let unavailable = outcome.unavailable.as_deref();
    let reason = screen_reason(&outcome.rules, unavailable);
    let kind = if unavailable.is_some() {
        SIDECAR_UNAVAILABLE_KIND
    } else {
        SECURITY_VERDICT_KIND
    };
    match outcome.verdict {
        ScreenVerdict::Block => Err(anyhow::anyhow!(
            "MCP endpoint {shown} refused by the security screen ({reason}); \
             endpoint URLs are screened before any connect (D-04d)"
        )),
        ScreenVerdict::Warn if !level.yolo_bypass_active() => Err(anyhow::anyhow!(
            "MCP endpoint {shown} needs approval ({reason}) and an MCP connect has no \
             approval prompt; refused at DEFCON {}",
            level.level()
        )),
        ScreenVerdict::Warn => {
            tracing::warn!(
                endpoint = %shown,
                rules = ?outcome.rules,
                unavailable = ?unavailable,
                "MCP endpoint flagged by the security screen; connecting"
            );
            write_screen_row(
                MCP_ENDPOINT_TOOL,
                kind,
                None,
                format!("{shown}: {reason}"),
                "warned",
                level.level(),
                &outcome.rules,
            );
            Ok(())
        }
        ScreenVerdict::Allow => {
            if unavailable.is_some() {
                write_screen_row(
                    MCP_ENDPOINT_TOOL,
                    kind,
                    None,
                    format!("{shown}: {reason}"),
                    "allow",
                    level.level(),
                    &outcome.rules,
                );
            }
            Ok(())
        }
    }
}

/// [`screen_endpoint_url_with`] against the process-wide screen at the process
/// default DEFCON level. With no screen installed (tests, an unbooted process) the
/// connect proceeds exactly as before.
pub fn screen_endpoint_url(url: &str) -> anyhow::Result<()> {
    match ironhermes_core::content_screen::content_screen() {
        Some(screen) => screen_endpoint_url_with(
            screen.as_ref(),
            url,
            ironhermes_core::defcon::session_defcon().process_default(),
        ),
        None => Ok(()),
    }
}

/// Test scaffolding for the process-wide content screen (Phase 57 plan 08).
///
/// The screen and the process DEFCON default are globals, so every test that
/// installs a fake takes [`ScreenTestGuard`], which serializes them, points audit
/// rows at a private temp file (never the real home) and restores the globals on
/// drop.
#[cfg(test)]
pub(crate) mod test_support {
    use ironhermes_core::config::DefconLevel;
    use ironhermes_core::content_screen::{
        ContentScreen, ContentScreenOutcome, ContentScreenRequest, ScreenVerdict,
        clear_content_screen_for_tests, install_content_screen,
    };
    use ironhermes_core::defcon::session_defcon;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};

    static SCREEN_TEST_LOCK: Mutex<()> = Mutex::new(());
    static AUDIT_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// The audit file override, when a test guard is alive.
    pub(crate) fn audit_path_override() -> Option<PathBuf> {
        AUDIT_PATH.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) struct ScreenTestGuard {
        _lock: MutexGuard<'static, ()>,
        dir: PathBuf,
    }

    impl ScreenTestGuard {
        pub(crate) fn new() -> Self {
            let lock = SCREEN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir().join(format!(
                "ironhermes_mcp_screen_{}_{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&dir).expect("test dir");
            *AUDIT_PATH.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(dir.join("logs").join("audit.jsonl"));
            clear_content_screen_for_tests();
            session_defcon().set_process_default(DefconLevel::THREE);
            Self { _lock: lock, dir }
        }

        pub(crate) fn install(&self, fake: &Arc<ScriptFake>) {
            install_content_screen(fake.clone());
        }

        pub(crate) fn set_level(&self, level: u8) {
            session_defcon().set_process_default(DefconLevel::try_from(level).unwrap());
        }

        /// Every JSON row written to the test audit file.
        pub(crate) fn audit_rows(&self) -> Vec<serde_json::Value> {
            std::fs::read_to_string(self.dir.join("logs").join("audit.jsonl"))
                .unwrap_or_default()
                .lines()
                .map(|l| serde_json::from_str(l).expect("audit row is JSON"))
                .collect()
        }
    }

    impl Drop for ScreenTestGuard {
        fn drop(&mut self) {
            clear_content_screen_for_tests();
            session_defcon().set_process_default(DefconLevel::THREE);
            *AUDIT_PATH.lock().unwrap_or_else(|e| e.into_inner()) = None;
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// A level-aware verdict for a matched needle.
    pub(crate) type VerdictAt = fn(u8) -> ScreenVerdict;

    pub(crate) fn block_always(_: u8) -> ScreenVerdict {
        ScreenVerdict::Block
    }

    /// Mirrors the High row of the policy table: Block at levels 1-3, Warn at 4-5.
    pub(crate) fn high(level: u8) -> ScreenVerdict {
        if level <= 3 {
            ScreenVerdict::Block
        } else {
            ScreenVerdict::Warn
        }
    }

    /// A scripted `ContentScreen` that records every request text it was asked about.
    pub(crate) struct ScriptFake {
        rules: Vec<(&'static str, &'static str, VerdictAt)>,
        unavailable: Option<&'static str>,
        pub(crate) seen: Mutex<Vec<String>>,
    }

    impl ScriptFake {
        pub(crate) fn new() -> Arc<Self> {
            Arc::new(Self {
                rules: Vec::new(),
                unavailable: None,
                seen: Mutex::new(Vec::new()),
            })
        }

        pub(crate) fn with_rule(
            needle: &'static str,
            rule: &'static str,
            verdict_at: VerdictAt,
        ) -> Arc<Self> {
            Arc::new(Self {
                rules: vec![(needle, rule, verdict_at)],
                unavailable: None,
                seen: Mutex::new(Vec::new()),
            })
        }

        pub(crate) fn unavailable(reason: &'static str) -> Arc<Self> {
            Arc::new(Self {
                rules: Vec::new(),
                unavailable: Some(reason),
                seen: Mutex::new(Vec::new()),
            })
        }

        pub(crate) fn seen_count(&self) -> usize {
            self.seen.lock().unwrap().len()
        }
    }

    impl ContentScreen for ScriptFake {
        fn screen(&self, req: &ContentScreenRequest<'_>) -> ContentScreenOutcome {
            self.seen.lock().unwrap().push(req.text.to_string());
            if let Some(reason) = self.unavailable {
                // Mirrors the real service: a Warn-class surface degrades by level,
                // anything else keeps the floor verdict (Allow), reason always set.
                let verdict = if !req.warn_class {
                    ScreenVerdict::Allow
                } else if req.level.level() >= 4 {
                    ScreenVerdict::Warn
                } else {
                    ScreenVerdict::Block
                };
                return ContentScreenOutcome {
                    verdict,
                    rules: Vec::new(),
                    max_severity: None,
                    unavailable: Some(reason.to_string()),
                };
            }
            for (needle, rule, verdict_at) in &self.rules {
                if req.text.contains(needle) {
                    return ContentScreenOutcome {
                        verdict: verdict_at(req.level.level()),
                        rules: vec![(*rule).to_string()],
                        max_severity: Some("HIGH".to_string()),
                        unavailable: None,
                    };
                }
            }
            ContentScreenOutcome {
                verdict: ScreenVerdict::Allow,
                rules: Vec::new(),
                max_severity: None,
                unavailable: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // build_safe_env tests (D-19)
    // -------------------------------------------------------------------------

    #[test]
    fn test_build_safe_env_includes_allowlisted_keys() {
        // These vars should be present on macOS/Linux test environments
        let user_env = HashMap::new();
        let env = build_safe_env(&user_env);

        // At minimum PATH and HOME should be present in most environments
        // We test the keys that are present rather than assuming all are set
        for key in &["PATH", "HOME"] {
            if std::env::var(key).is_ok() {
                assert!(
                    env.contains_key(*key),
                    "Expected safe env to include {key} since it exists in host env"
                );
            }
        }
    }

    #[test]
    fn test_build_safe_env_includes_xdg_vars() {
        // Inject a test XDG var to verify XDG_* prefix filtering
        // SAFETY: test-only env mutation; single-threaded via --test-threads=1 convention
        unsafe { std::env::set_var("XDG_TEST_MCP_VAR", "xdg_test_value") };
        let user_env = HashMap::new();
        let env = build_safe_env(&user_env);
        assert_eq!(
            env.get("XDG_TEST_MCP_VAR").map(|s| s.as_str()),
            Some("xdg_test_value"),
            "XDG_* vars from host env should be included"
        );
        unsafe { std::env::remove_var("XDG_TEST_MCP_VAR") };
    }

    #[test]
    fn test_build_safe_env_excludes_credential_vars() {
        // Inject credential vars and verify they are excluded.
        // SAFETY: test-only env mutation; these keys are not used by other tests.
        unsafe {
            std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test");
            std::env::set_var("AWS_SECRET_ACCESS_KEY", "aws-secret-test");
            std::env::set_var("OPENAI_API_KEY", "sk-openai-test");
        }

        let user_env = HashMap::new();
        let env = build_safe_env(&user_env);

        assert!(
            !env.contains_key("ANTHROPIC_API_KEY"),
            "ANTHROPIC_API_KEY must be excluded from safe env"
        );
        assert!(
            !env.contains_key("AWS_SECRET_ACCESS_KEY"),
            "AWS_SECRET_ACCESS_KEY must be excluded from safe env"
        );
        assert!(
            !env.contains_key("OPENAI_API_KEY"),
            "OPENAI_API_KEY must be excluded from safe env"
        );

        unsafe {
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::remove_var("AWS_SECRET_ACCESS_KEY");
            std::env::remove_var("OPENAI_API_KEY");
        }
    }

    #[test]
    fn test_build_safe_env_includes_user_overrides() {
        let mut user_env = HashMap::new();
        user_env.insert("GITHUB_TOKEN".to_string(), "ghp_user_token".to_string());
        user_env.insert("MY_CUSTOM_VAR".to_string(), "custom_value".to_string());

        let env = build_safe_env(&user_env);

        assert_eq!(
            env.get("GITHUB_TOKEN").map(|s| s.as_str()),
            Some("ghp_user_token"),
            "User-specified env vars should be included"
        );
        assert_eq!(
            env.get("MY_CUSTOM_VAR").map(|s| s.as_str()),
            Some("custom_value"),
            "User-specified env vars should be included"
        );
    }

    #[test]
    fn test_build_safe_env_user_overrides_take_precedence() {
        // User env should override the host allowlisted value
        let mut user_env = HashMap::new();
        user_env.insert("PATH".to_string(), "/custom/path".to_string());

        let env = build_safe_env(&user_env);
        assert_eq!(
            env.get("PATH").map(|s| s.as_str()),
            Some("/custom/path"),
            "User-specified env should override host env for same key"
        );
    }

    // -------------------------------------------------------------------------
    // sanitize_error tests (D-20)
    // -------------------------------------------------------------------------

    /// T-54.8-49: a callback URL quoted in an exchange error must not leak its
    /// one-time code or PKCE state through the sanitized text.
    #[test]
    fn sanitize_error_masks_oauth_values() {
        let result = sanitize_error(
            "exchange failed for http://127.0.0.1:8765/callback?code=cd-123&state=st-1",
        );
        assert!(!result.contains("cd-123"), "code leaked: {result}");
        assert!(!result.contains("st-1"), "state leaked: {result}");
    }

    #[test]
    fn test_sanitize_error_redacts_github_token() {
        let text = "Error connecting: ghp_abc123XYZ token rejected";
        let result = sanitize_error(text);
        assert!(
            result.contains("[REDACTED]"),
            "ghp_ tokens should be redacted: {result}"
        );
        assert!(
            !result.contains("ghp_abc123XYZ"),
            "Original token must not appear in output"
        );
    }

    #[test]
    fn test_sanitize_error_redacts_sk_token() {
        let text = "API call failed with key sk-abc123DEF456 invalid";
        let result = sanitize_error(text);
        assert!(
            result.contains("[REDACTED]"),
            "sk- tokens should be redacted: {result}"
        );
        assert!(!result.contains("sk-abc123DEF456"));
    }

    #[test]
    fn test_sanitize_error_redacts_bearer_token() {
        let text = "Unauthorized: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig";
        let result = sanitize_error(text);
        assert!(
            result.contains("[REDACTED]"),
            "Bearer tokens should be redacted: {result}"
        );
        assert!(!result.contains("eyJhbGciOiJIUzI1NiJ9"));
    }

    #[test]
    fn test_sanitize_error_redacts_token_assignment() {
        let text = "Request with token=supersecretvalue123 failed";
        let result = sanitize_error(text);
        assert!(
            result.contains("[REDACTED]"),
            "token= assignments should be redacted: {result}"
        );
        assert!(!result.contains("supersecretvalue123"));
    }

    #[test]
    fn test_sanitize_error_redacts_key_assignment() {
        let text = "Request with key=mysecretkey456 failed";
        let result = sanitize_error(text);
        assert!(
            result.contains("[REDACTED]"),
            "key= assignments should be redacted: {result}"
        );
        assert!(!result.contains("mysecretkey456"));
    }

    #[test]
    fn test_sanitize_error_redacts_api_key_assignment() {
        let text = "Auth header API_KEY=abcdefg12345 not accepted";
        let result = sanitize_error(text);
        assert!(
            result.contains("[REDACTED]"),
            "API_KEY= should be redacted: {result}"
        );
        assert!(!result.contains("abcdefg12345"));
    }

    #[test]
    fn test_sanitize_error_redacts_password_assignment() {
        let text = "Connection refused: password=hunter2 wrong";
        let result = sanitize_error(text);
        assert!(
            result.contains("[REDACTED]"),
            "password= should be redacted: {result}"
        );
        assert!(!result.contains("hunter2"));
    }

    #[test]
    fn test_sanitize_error_redacts_secret_assignment() {
        let text = "Validation failed: secret=topsecret123 mismatch";
        let result = sanitize_error(text);
        assert!(
            result.contains("[REDACTED]"),
            "secret= should be redacted: {result}"
        );
        assert!(!result.contains("topsecret123"));
    }

    #[test]
    fn test_sanitize_error_preserves_non_credential_text() {
        let text = "Connection refused: server at localhost:8080 is not running";
        let result = sanitize_error(text);
        assert_eq!(
            result, text,
            "Non-credential error text should pass through unchanged"
        );
    }

    #[test]
    fn test_sanitize_error_empty_string() {
        assert_eq!(sanitize_error(""), "");
    }

    #[test]
    fn test_sanitize_error_multiple_credentials() {
        let text = "Token ghp_abc123 and key=mysecret both invalid";
        let result = sanitize_error(text);
        assert!(!result.contains("ghp_abc123"));
        assert!(!result.contains("mysecret"));
        // Should have two [REDACTED] replacements
        assert_eq!(result.matches("[REDACTED]").count(), 2);
    }

    // -------------------------------------------------------------------------
    // is_oauth_reauthorization_required tests (warm-but-revoked follow-up fix)
    // -------------------------------------------------------------------------

    #[test]
    fn test_is_oauth_reauthorization_required_matches_get_access_token_prefix() {
        // The exact text transport.rs's hot path produces when a cached token's
        // refresh call fails (e.g. the AS returns invalid_grant).
        let reason = "Get access token: token refresh failed: invalid_grant: Grant not found";
        assert!(is_oauth_reauthorization_required(reason));
    }

    #[test]
    fn test_is_oauth_reauthorization_required_matches_insufficient_scope_display() {
        let reason = McpOAuthErrorDisplayFixture::insufficient_scope();
        assert!(is_oauth_reauthorization_required(&reason));
    }

    #[test]
    fn test_is_oauth_reauthorization_required_matches_invalid_token_retry_exhausted_display() {
        let reason = McpOAuthErrorDisplayFixture::invalid_token_retry_exhausted();
        assert!(is_oauth_reauthorization_required(&reason));
    }

    #[test]
    fn test_is_oauth_reauthorization_required_matches_401_at_discovery() {
        let reason = "OAuth 401 at discovery — invalid_token";
        assert!(is_oauth_reauthorization_required(reason));
    }

    #[test]
    fn test_is_oauth_reauthorization_required_false_for_genuine_transport_failure() {
        for reason in [
            "connection refused",
            "connection failed after retries",
            "MCP tool call timed out after 30s",
            "Failed to accept loopback callback: os error 24",
        ] {
            assert!(
                !is_oauth_reauthorization_required(reason),
                "expected {reason:?} to NOT classify as auth-caused"
            );
        }
    }

    #[test]
    fn test_is_oauth_reauthorization_required_false_for_empty_string() {
        assert!(!is_oauth_reauthorization_required(""));
    }

    /// Fixture reproducing the exact `Display` text of the two `McpOAuthError`
    /// variants (`server_task.rs`) without creating a circular dev-dependency
    /// on that module's private construction — the strings are copied
    /// verbatim from `server_task.rs`'s `impl std::fmt::Display for
    /// McpOAuthError` and must be kept in sync if that text ever changes.
    struct McpOAuthErrorDisplayFixture;
    impl McpOAuthErrorDisplayFixture {
        fn insufficient_scope() -> String {
            "MCP server 'srv': OAuth insufficient_scope (permanent — \
             run `hermes mcp connect srv` to re-authorize with correct scopes)"
                .to_string()
        }
        fn invalid_token_retry_exhausted() -> String {
            "MCP server 'srv': OAuth token refresh + 1-retry both returned 401 \
             (permanent — run `hermes mcp connect srv` to re-authorize)"
                .to_string()
        }
    }

    // -------------------------------------------------------------------------
    // validate_prm_issuer tests (B-4, D-07, D-01)
    // -------------------------------------------------------------------------

    /// Baseline-only allowed set — mirrors the pre-46.1 hardcoded const, used by
    /// every pre-existing anti-bypass test so their assertions are unchanged.
    fn baseline() -> Vec<String> {
        BASELINE_ISSUER_ALLOWLIST
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn test_prm_issuer_rejects_http() {
        assert!(
            validate_prm_issuer("http://cloudflare.com/auth", &baseline()).is_err(),
            "Non-HTTPS issuer URLs must be rejected (B-4)"
        );
    }

    #[test]
    fn test_prm_issuer_rejects_off_allowlist() {
        assert!(
            validate_prm_issuer("https://evil.example.com/auth", &baseline()).is_err(),
            "Issuers not on the allowlist must be rejected (B-4)"
        );
    }

    #[test]
    fn test_prm_issuer_accepts_cloudflare() {
        assert!(
            validate_prm_issuer("https://dash.cloudflare.com/oauth", &baseline()).is_ok(),
            "Cloudflare HTTPS issuer must be accepted (B-4)"
        );
    }

    #[test]
    fn test_prm_issuer_accepts_apex_cloudflare() {
        assert!(
            validate_prm_issuer("https://cloudflare.com/oauth", &baseline()).is_ok(),
            "Apex cloudflare.com HTTPS issuer must be accepted (B-4)"
        );
    }

    // SSRF allowlist-bypass regression tests (B-4). The previous implementation
    // used `issuer_url.contains(domain)`, which accepted all of the following.

    #[test]
    fn test_prm_issuer_rejects_prefix_bypass() {
        // Host has the allowlisted domain as a substring but a different registrable name.
        assert!(
            validate_prm_issuer("https://attacker-cloudflare.com/auth", &baseline()).is_err(),
            "Prefix-substring host must NOT bypass the allowlist (B-4)"
        );
        assert!(
            validate_prm_issuer("https://notcloudflare.com/auth", &baseline()).is_err(),
            "Substring-without-dot host must NOT bypass the allowlist (B-4)"
        );
    }

    #[test]
    fn test_prm_issuer_rejects_fake_suffix_bypass() {
        // Allowlisted label appears as a left-hand subdomain of an attacker domain.
        assert!(
            validate_prm_issuer("https://cloudflare.com.evil.com/auth", &baseline()).is_err(),
            "Allowlisted label as a subdomain of an attacker domain must be rejected (B-4)"
        );
    }

    #[test]
    fn test_prm_issuer_rejects_allowlisted_in_path_or_query() {
        // Allowlisted domain appears only in the path/query, not the host.
        assert!(
            validate_prm_issuer("https://evil.com/cloudflare.com", &baseline()).is_err(),
            "Allowlisted domain in the path must not be accepted (B-4)"
        );
        assert!(
            validate_prm_issuer(
                "https://evil.com/?redirect=https://dash.cloudflare.com",
                &baseline()
            )
            .is_err(),
            "Allowlisted domain in the query must not be accepted (B-4)"
        );
    }

    #[test]
    fn test_prm_issuer_rejects_allowlisted_in_userinfo() {
        // Allowlisted domain placed in the userinfo component; real host is evil.com.
        assert!(
            validate_prm_issuer("https://dash.cloudflare.com@evil.com/auth", &baseline()).is_err(),
            "Allowlisted domain in userinfo must not be accepted — real host is evil.com (B-4)"
        );
    }

    #[test]
    fn test_prm_issuer_rejects_malformed_url() {
        assert!(
            validate_prm_issuer("not a url", &baseline()).is_err(),
            "Malformed issuer URLs must be rejected (B-4)"
        );
        assert!(
            validate_prm_issuer("https://", &baseline()).is_err(),
            "URL with no host must be rejected (B-4)"
        );
    }

    // -------------------------------------------------------------------------
    // resolve_allowed_issuers / D-01 layered-resolution tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_prm_issuer_per_server_pin_authoritative() {
        // Pin accepts its own host.
        let allowed = resolve_allowed_issuers(Some("github.com"), &["evil.com".to_string()]);
        assert_eq!(allowed, vec!["github.com".to_string()]);
        assert!(
            validate_prm_issuer("https://github.com/oauth", &allowed).is_ok(),
            "A per-server pin must accept its own declared issuer host"
        );

        // Pin rejects a global-list member — the global list is NOT consulted when a pin is set.
        assert!(
            validate_prm_issuer("https://evil.com/oauth", &allowed).is_err(),
            "A per-server pin must reject a global-additive-list member — global list is not \
             consulted when a pin is present (D-01)"
        );

        // Pin rejects baseline too — a pin replaces, never widens, the allowed set.
        assert!(
            validate_prm_issuer("https://cloudflare.com/oauth", &allowed).is_err(),
            "A per-server pin must reject the baseline (cloudflare.com) when the pin names a \
             different host — a pin is authoritative, not additive (D-01)"
        );
    }

    #[test]
    fn test_prm_issuer_global_additive_accepts_configured() {
        // No pin + a configured non-Cloudflare issuer in the global list → accepted.
        let allowed = resolve_allowed_issuers(None, &["github.com".to_string()]);
        assert_eq!(
            allowed,
            vec![
                "cloudflare.com".to_string(),
                "dash.cloudflare.com".to_string(),
                "github.com".to_string(),
            ],
            "resolve_allowed_issuers(None, global) must be baseline ∪ global additive"
        );
        assert!(
            validate_prm_issuer("https://github.com/oauth", &allowed).is_ok(),
            "A configured non-Cloudflare issuer in the global additive list must be accepted \
             when no per-server pin is set (D-01)"
        );

        // Baseline still accepted with zero new config (CFL-02 no-regression).
        assert!(
            validate_prm_issuer("https://dash.cloudflare.com/oauth", &allowed).is_ok(),
            "Baseline issuers must remain accepted — the global list is additive, not \
             replacing (CFL-02)"
        );

        // An issuer neither pinned, baseline, nor in the global list is still rejected.
        assert!(
            validate_prm_issuer("https://evil.example.com/oauth", &allowed).is_err(),
            "An issuer absent from pin/baseline/global must still be rejected (B-4)"
        );

        // github.com is NOT accepted when the global list is empty (baseline-only).
        let baseline_only = resolve_allowed_issuers(None, &[]);
        assert!(
            validate_prm_issuer("https://github.com/oauth", &baseline_only).is_err(),
            "Without a configured global entry, a non-baseline issuer must be rejected"
        );
    }

    #[test]
    fn test_prm_issuer_empty_pin_treated_as_absent() {
        // V5: whitespace-only pin must fall back to baseline ∪ global, not an empty allowlist.
        let allowed = resolve_allowed_issuers(Some("   "), &["github.com".to_string()]);
        assert_eq!(
            allowed,
            vec![
                "cloudflare.com".to_string(),
                "dash.cloudflare.com".to_string(),
                "github.com".to_string(),
            ],
            "An empty/whitespace-only pin must be treated as absent (fail-safe fallback)"
        );
    }

    // -------------------------------------------------------------------------
    // is_baseline_issuer tests (D-02 seam)
    // -------------------------------------------------------------------------

    #[test]
    fn test_is_baseline_issuer_true_for_baseline_hosts() {
        assert!(is_baseline_issuer("https://dash.cloudflare.com/x"));
        assert!(is_baseline_issuer("https://cloudflare.com/x"));
    }

    #[test]
    fn test_is_baseline_issuer_false_for_non_baseline() {
        assert!(!is_baseline_issuer("https://github.com/x"));
        assert!(!is_baseline_issuer("not a url"));
    }

    // -------------------------------------------------------------------------
    // validate_web_redirect_base tests (D-13 / T-48.2-08-02)
    // -------------------------------------------------------------------------

    #[test]
    fn test_validate_web_redirect_base_accepts_bare_https_origin() {
        let result = validate_web_redirect_base("https://hermes.example.com");
        assert_eq!(result, Ok("https://hermes.example.com".to_string()));
    }

    #[test]
    fn test_validate_web_redirect_base_accepts_bare_https_origin_with_trailing_slash() {
        let result = validate_web_redirect_base("https://hermes.example.com/");
        assert_eq!(result, Ok("https://hermes.example.com".to_string()));
    }

    #[test]
    fn test_validate_web_redirect_base_accepts_http_origin_with_explicit_port() {
        let result = validate_web_redirect_base("http://192.0.2.10:8080");
        assert_eq!(result, Ok("http://192.0.2.10:8080".to_string()));
    }

    #[test]
    fn test_validate_web_redirect_base_rejects_non_http_scheme() {
        assert!(validate_web_redirect_base("ftp://hermes.example.com").is_err());
        assert!(validate_web_redirect_base("javascript://hermes.example.com").is_err());
    }

    #[test]
    fn test_validate_web_redirect_base_rejects_absent_or_empty_host() {
        // "https://" has no authority at all — host_str() is None.
        assert!(validate_web_redirect_base("https://").is_err());
        assert!(validate_web_redirect_base("not a url").is_err());
    }

    #[test]
    fn test_validate_web_redirect_base_rejects_userinfo() {
        assert!(validate_web_redirect_base("https://user@hermes.example.com").is_err());
        assert!(validate_web_redirect_base("https://user:pass@hermes.example.com").is_err());
    }

    #[test]
    fn test_validate_web_redirect_base_rejects_non_root_path() {
        assert!(validate_web_redirect_base("https://hermes.example.com/oauth").is_err());
    }

    #[test]
    fn test_validate_web_redirect_base_rejects_query_string() {
        assert!(validate_web_redirect_base("https://hermes.example.com?x=1").is_err());
    }

    #[test]
    fn test_validate_web_redirect_base_rejects_fragment() {
        assert!(validate_web_redirect_base("https://hermes.example.com#frag").is_err());
    }

    #[test]
    fn test_validate_web_redirect_base_rejects_over_length_input() {
        let long_host = "a".repeat(300);
        let input = format!("https://{long_host}.example.com");
        assert!(validate_web_redirect_base(&input).is_err());
    }

    /// Phase 48.2 Plan 08 Task 2: every rejection class must produce a FIXED
    /// error message that never echoes any part of the input — the same
    /// "never echo an input field" discipline `perform_oauth_connect`'s doc
    /// comment states for the web layer, and `sanitize_error` enforces
    /// elsewhere in this module.
    #[test]
    fn test_validate_web_redirect_base_errors_never_echo_input() {
        let marker = "UNIQUE_MARKER_ZzQ7f3";

        // One case per rejection class from validate_web_redirect_base's doc
        // comment, each carrying `marker` somewhere an echoing implementation
        // would leak it.
        let cases: Vec<String> = vec![
            format!("ftp://{marker}.example.com"), // non-http(s) scheme
            format!("not a url {marker}"),         // malformed / no host
            format!("https://{marker}@hermes.example.com"), // userinfo (username)
            format!("https://user:{marker}@hermes.example.com"), // userinfo (password)
            format!("https://hermes.example.com/{marker}"), // non-root path
            format!("https://hermes.example.com?x={marker}"), // query string
            format!("https://hermes.example.com#{marker}"), // fragment
            format!("https://{marker}{}.example.com", "a".repeat(300)), // over-length
        ];

        for input in &cases {
            let result = validate_web_redirect_base(input);
            assert!(
                result.is_err(),
                "expected validate_web_redirect_base to reject input={input:?}"
            );
            let err = result.unwrap_err();
            assert!(
                !err.contains(marker),
                "validate_web_redirect_base error message must never echo the input value; \
                 got err={err:?} for input={input:?}"
            );
        }
    }

    // -------------------------------------------------------------------------
    // Phase 57 plan 08 (D-04d / D-12): screen_endpoint_url_with
    // -------------------------------------------------------------------------

    use super::test_support::{ScreenTestGuard, ScriptFake, block_always, high};
    use ironhermes_core::config::DefconLevel;

    fn lvl(n: u8) -> DefconLevel {
        DefconLevel::try_from(n).unwrap()
    }

    #[test]
    fn redact_url_keeps_only_the_origin() {
        assert_eq!(
            redact_url("https://user:pw@mcp.example.com:8443/a/b?token=abc#frag"),
            "https://mcp.example.com:8443"
        );
        assert_eq!(
            redact_url("http://169.254.169.254/x"),
            "http://169.254.169.254"
        );
        assert_eq!(redact_url("not a url sekret"), "<unparseable url>");
    }

    #[test]
    fn endpoint_text_is_a_quoted_curl_command_with_quotes_escaped() {
        let _g = ScreenTestGuard::new();
        let fake = ScriptFake::new();
        screen_endpoint_url_with(fake.as_ref(), "https://h.example.com/a'b", lvl(3)).unwrap();
        assert_eq!(
            fake.seen.lock().unwrap().clone(),
            vec![r"curl 'https://h.example.com/a'\''b'".to_string()]
        );
    }

    #[test]
    fn endpoint_block_error_names_rules_and_hides_credentials() {
        let _g = ScreenTestGuard::new();
        let fake = ScriptFake::with_rule("evil", "MetadataEndpoint", block_always);
        let err = screen_endpoint_url_with(
            fake.as_ref(),
            "https://user:pw@evil.example.com/p?token=sekret",
            lvl(5),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("MetadataEndpoint"), "{err}");
        assert!(err.contains("https://evil.example.com"), "{err}");
        for leaked in ["user", "pw@", "sekret", "/p?"] {
            assert!(!err.contains(leaked), "leaked {leaked:?} in {err}");
        }
    }

    #[test]
    fn endpoint_warn_refused_at_one_to_three_allowed_with_row_at_four_and_five() {
        let g = ScreenTestGuard::new();
        let fake = ScriptFake::with_rule("xn--", "PunycodeDomain", |_| {
            ironhermes_core::content_screen::ScreenVerdict::Warn
        });
        for n in 1..=3 {
            let err = screen_endpoint_url_with(fake.as_ref(), "https://xn--a.example.com", lvl(n))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("approval") && err.contains("PunycodeDomain"),
                "level {n}: {err}"
            );
        }
        assert!(g.audit_rows().is_empty(), "a refusal writes no verdict row");
        for n in 4..=5 {
            screen_endpoint_url_with(fake.as_ref(), "https://xn--a.example.com", lvl(n)).unwrap();
        }
        let rows = g.audit_rows();
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(rows.iter().all(|r| r["kind"] == "security_verdict"
            && r["tool"] == "mcp_endpoint"
            && r["decision"] == "warned"));
        assert!(
            !serde_json::to_string(&rows)
                .unwrap()
                .contains("xn--a.example.com/"),
            "rows carry no URL path"
        );
    }

    #[test]
    fn endpoint_unavailable_follows_the_warn_class_rule() {
        let g = ScreenTestGuard::new();
        let fake = ScriptFake::unavailable("timeout");
        let err = screen_endpoint_url_with(fake.as_ref(), "https://h.example.com", lvl(3))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("sidecar unavailable") && err.contains("timeout"),
            "{err}"
        );
        screen_endpoint_url_with(fake.as_ref(), "https://h.example.com", lvl(4)).unwrap();
        let rows = g.audit_rows();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["kind"], "sidecar_unavailable");
        assert_eq!(rows[0]["tool"], "mcp_endpoint");
        assert!(rows[0]["reason"].as_str().unwrap().contains("timeout"));
    }

    #[test]
    fn endpoint_clean_verdict_is_ok_and_silent() {
        let g = ScreenTestGuard::new();
        let fake = ScriptFake::with_rule("never-matches", "X", high);
        for n in 1..=5 {
            screen_endpoint_url_with(fake.as_ref(), "https://mcp.example.com/mcp", lvl(n)).unwrap();
        }
        assert!(g.audit_rows().is_empty());
    }
}
