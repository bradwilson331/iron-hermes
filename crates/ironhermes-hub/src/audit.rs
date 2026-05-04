//! Pre-install audit endpoint client (D-19).
//!
//! Fetches partner-risk data from `https://add-skill.vercel.sh/audit?source=<owner/repo>&skills=<slugs>`
//! with a 3-second timeout. Every error path is soft-fail: return Option::None and
//! tracing::warn — the installer NEVER refuses to install because audit is down.
//!
//! `--skip-audit` is honored at the caller (skills_cmd.rs) by simply not calling `fetch_audit`.

use crate::sanitize::strip_terminal_escapes;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

const DEFAULT_AUDIT_BASE_URL: &str = "https://add-skill.vercel.sh";
const AUDIT_TIMEOUT: Duration = Duration::from_secs(3); // D-19

fn resolve_audit_base_url() -> String {
    std::env::var("SKILLS_AUDIT_URL").unwrap_or_else(|_| DEFAULT_AUDIT_BASE_URL.to_string())
}

/// Partner audit data for a single skill slug.
#[derive(Debug, Clone, Deserialize)]
pub struct PartnerAudit {
    /// "safe" | "low" | "medium" | "high" | "critical" | "unknown"
    pub risk: String,
    #[serde(default)]
    pub alerts: u64,
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(default, rename = "analyzedAt")]
    pub analyzed_at: Option<String>,
}

/// Audit response keyed by skill slug.
pub type AuditData = HashMap<String, PartnerAudit>;

/// Fetch audit data for `<owner>/<repo>` + skill slug(s). Returns None on any error
/// (timeout, 5xx, non-JSON, DNS) — installer must proceed with install regardless.
///
/// Every user-facing log line is run through `strip_terminal_escapes` before emit.
#[tracing::instrument(skip(http))]
pub async fn fetch_audit(
    http: &reqwest::Client,
    owner_repo: &str,
    skill_slugs: &[&str],
) -> Option<AuditData> {
    let base = resolve_audit_base_url();
    let joined = skill_slugs.join(",");
    let url = format!("{}/audit", base.trim_end_matches('/'));
    // Use reqwest::RequestBuilder::query for URL encoding — no new crates.
    let request = http
        .get(&url)
        .query(&[("source", owner_repo), ("skills", joined.as_str())]);

    match tokio::time::timeout(AUDIT_TIMEOUT, request.send()).await {
        Ok(Ok(resp)) if resp.status().is_success() => match resp.json::<AuditData>().await {
            Ok(data) => Some(data),
            Err(e) => {
                let msg = strip_terminal_escapes(&format!("{e}"));
                tracing::warn!(
                    audit_url = %url,
                    error = %msg,
                    "audit parse failed — soft-fail, proceeding without risk data"
                );
                None
            }
        },
        Ok(Ok(resp)) => {
            let status = resp.status();
            tracing::warn!(
                audit_url = %url,
                %status,
                "audit endpoint returned non-success — soft-fail"
            );
            None
        }
        Ok(Err(e)) => {
            let msg = strip_terminal_escapes(&format!("{e}"));
            tracing::warn!(audit_url = %url, error = %msg, "audit HTTP error — soft-fail");
            None
        }
        Err(_timeout) => {
            tracing::warn!(
                audit_url = %url,
                timeout_secs = 3,
                "audit endpoint timed out — soft-fail"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Guard to set SKILLS_AUDIT_URL for the duration of an async test.
    /// The guard must outlive all awaits that depend on the env var. Drop
    /// restores the previous value.
    struct AuditUrlGuard {
        prev: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl AuditUrlGuard {
        fn set(url: &str) -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var("SKILLS_AUDIT_URL").ok();
            unsafe {
                std::env::set_var("SKILLS_AUDIT_URL", url);
            }
            Self { prev, _lock: lock }
        }
    }

    impl Drop for AuditUrlGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var("SKILLS_AUDIT_URL", v),
                    None => std::env::remove_var("SKILLS_AUDIT_URL"),
                }
            }
        }
    }

    fn http_client() -> reqwest::Client {
        reqwest::Client::builder().build().unwrap()
    }

    #[tokio::test]
    async fn happy_path_returns_some() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/audit"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ascii-art": {"risk": "low", "alerts": 0, "score": 8.5}
            })))
            .mount(&server)
            .await;

        let _g = AuditUrlGuard::set(&server.uri());
        let c = http_client();
        let result = fetch_audit(&c, "foo/bar", &["ascii-art"]).await;
        let data = result.expect("should return Some");
        assert_eq!(data.get("ascii-art").map(|a| a.risk.as_str()), Some("low"));
    }

    #[tokio::test]
    async fn timeout_soft_fails_to_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/audit"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(10)))
            .mount(&server)
            .await;

        let _g = AuditUrlGuard::set(&server.uri());
        let c = http_client();
        let result = fetch_audit(&c, "o/r", &["x"]).await;
        assert!(result.is_none(), "timeout must soft-fail to None");
    }

    #[tokio::test]
    async fn status_5xx_soft_fails() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/audit"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let _g = AuditUrlGuard::set(&server.uri());
        let c = http_client();
        let result = fetch_audit(&c, "o/r", &["x"]).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn non_json_soft_fails() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/audit"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<<<not json>>>"))
            .mount(&server)
            .await;

        let _g = AuditUrlGuard::set(&server.uri());
        let c = http_client();
        let result = fetch_audit(&c, "o/r", &["x"]).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn base_url_override_via_env() {
        // Prove that SKILLS_AUDIT_URL actually steers the request, by only mounting
        // a handler at the env-specified mock server.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/audit"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "x": {"risk": "safe", "alerts": 0}
            })))
            .mount(&server)
            .await;

        let _g = AuditUrlGuard::set(&server.uri());
        let c = http_client();
        let result = fetch_audit(&c, "o/r", &["x"]).await;
        assert!(result.is_some(), "override should route to mock");
    }
}
