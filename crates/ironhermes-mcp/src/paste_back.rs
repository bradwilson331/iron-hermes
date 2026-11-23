//! Paste-back MCP OAuth: the shared core for headless hosts (54.8-02, D-14, D-15).
//!
//! Inside a rootless container (or over SSH) a browser cannot reach the loopback
//! callback the normal PKCE flow listens on. Paste-back parks the authorization in
//! [`McpManager`] (the same `begin_oauth` / `complete_oauth` pair the web UI uses),
//! shows the operator the authorization link, and finishes the exchange from the
//! redirected address-bar URL the operator pastes back. No listener is bound and no
//! port is published.
//!
//! The CLI (`ironhermes mcp connect --no-browser`) and the Telegram `/mcp connect`
//! path (plan 09) both drive [`OAuthPasteBack`], so there is one implementation. The
//! callback detectors and the redactor are plan 16's [`crate::oauth_redact`], re-exported
//! here so a consumer imports one path.

use std::time::Duration;

use crate::config::McpServerConfig;
use crate::manager::{McpManager, OAuthAuthorizationStart};

pub use crate::oauth_redact::{
    find_oauth_callback_in_text, is_oauth_callback_url, is_oauth_denial, redact_oauth_params,
};

/// Redirect URI registered for paste-back authorizations.
///
/// An RFC 8252 loopback URI that nothing listens on: after approving, the browser
/// fails to load it and the operator copies the address bar. Single-sourced so the
/// live Cloudflare drill (acceptance A-3, RESEARCH A2: does dynamic client
/// registration accept a fixed loopback redirect?) can change it in one place.
pub const MCP_PASTE_BACK_REDIRECT_URI: &str = "http://127.0.0.1:8765/callback";

/// Upper bound on starting an authorization (discovery + dynamic client registration).
/// Same bound the web connect uses.
pub const PASTE_BACK_BEGIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound on completing an authorization (token exchange) from a pasted URL.
pub const PASTE_BACK_COMPLETE_TIMEOUT: Duration = Duration::from_secs(60);

/// How long [`OAuthPasteBack::reconnect_server`] waits for the restarted server to
/// report connected before summarizing it as "not connected yet".
const RECONNECT_WAIT: Duration = Duration::from_secs(10);
const RECONNECT_POLL: Duration = Duration::from_millis(250);

/// The operations a paste-back surface needs from the MCP layer.
///
/// All error strings are already sanitized by the implementation; a consumer must
/// still never echo the pasted callback URL itself.
#[async_trait::async_trait]
pub trait OAuthPasteBack: Send + Sync {
    /// Start an authorization for `server_name` and park it. Time-bounded.
    async fn begin(
        &self,
        server_name: &str,
        config: &McpServerConfig,
    ) -> Result<OAuthAuthorizationStart, String>;

    /// Finish a parked authorization from a pasted callback URL (single use).
    /// Returns the server name. Time-bounded.
    async fn complete(&self, callback_url: &str) -> Result<String, String>;

    /// Discard a parked authorization (empty paste, denial, failure).
    async fn cancel(&self, state: &str);

    /// Reconnect ONLY the named server so it loads the freshly stored token, and
    /// return a one-line summary that carries names and counts, never error detail.
    async fn reconnect_server(&self, server_name: &str, config: &McpServerConfig) -> String;
}

#[async_trait::async_trait]
impl OAuthPasteBack for McpManager {
    async fn begin(
        &self,
        server_name: &str,
        config: &McpServerConfig,
    ) -> Result<OAuthAuthorizationStart, String> {
        match tokio::time::timeout(
            PASTE_BACK_BEGIN_TIMEOUT,
            self.begin_oauth(server_name, config, MCP_PASTE_BACK_REDIRECT_URI),
        )
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => Err("authorization could not be started within 30 s".to_string()),
        }
    }

    async fn complete(&self, callback_url: &str) -> Result<String, String> {
        match tokio::time::timeout(
            PASTE_BACK_COMPLETE_TIMEOUT,
            self.complete_oauth(callback_url),
        )
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => Err("authorization did not complete within 60 s".to_string()),
        }
    }

    async fn cancel(&self, state: &str) {
        self.cancel_oauth(state).await;
    }

    async fn reconnect_server(&self, server_name: &str, config: &McpServerConfig) -> String {
        self.restart_server(server_name, config.clone()).await;

        let deadline = tokio::time::Instant::now() + RECONNECT_WAIT;
        let connected = loop {
            if self
                .connected_server_names()
                .iter()
                .any(|n| n == server_name)
            {
                break true;
            }
            if tokio::time::Instant::now() >= deadline {
                break false;
            }
            tokio::time::sleep(RECONNECT_POLL).await;
        };

        if connected {
            let tools = self.registered_tool_count().await;
            format!("{server_name}: connected ({tools} MCP tools registered in total)")
        } else {
            format!("{server_name}: not connected yet")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_uri_is_a_loopback_url_that_is_not_itself_a_callback() {
        // The bare redirect URI carries no code/state, so it can never be mistaken
        // for a pasted callback by the detectors.
        assert!(MCP_PASTE_BACK_REDIRECT_URI.starts_with("http://127.0.0.1:"));
        assert!(!is_oauth_callback_url(MCP_PASTE_BACK_REDIRECT_URI));
        assert!(!is_oauth_denial(MCP_PASTE_BACK_REDIRECT_URI));
    }

    #[test]
    fn completion_bound_exceeds_begin_bound() {
        assert_eq!(PASTE_BACK_BEGIN_TIMEOUT, Duration::from_secs(30));
        assert_eq!(PASTE_BACK_COMPLETE_TIMEOUT, Duration::from_secs(60));
    }
}
