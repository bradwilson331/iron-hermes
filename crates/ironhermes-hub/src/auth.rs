//! GitHub token resolution (D-03).
//!
//! Precedence: explicit env override → HERMES_GITHUB_TOKEN → GITHUB_TOKEN →
//! `gh auth token` subprocess (2s timeout) → anonymous.

use std::fmt;

pub struct GitHubAuth {
    token: Option<String>,
}

impl GitHubAuth {
    /// Resolve a GitHub token using the D-03 precedence order.
    ///
    /// `override_env`: optional name of an environment variable that takes
    /// priority over the built-in defaults (comes from `config.skills.hub.github_token_env`).
    pub async fn resolve(override_env: Option<&str>) -> Self {
        if let Some(t) = Self::resolve_from_env(override_env, |k| std::env::var(k).ok()) {
            return Self { token: Some(t) };
        }

        if let Ok(Ok(out)) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            tokio::process::Command::new("gh")
                .args(["auth", "token"])
                .output(),
        )
        .await
        {
            if out.status.success() {
                let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !t.is_empty() {
                    return Self { token: Some(t) };
                }
            }
        }

        Self { token: None }
    }

    /// Pure env-only resolution (testable without subprocess).
    pub fn resolve_from_env<F>(override_env: Option<&str>, getenv: F) -> Option<String>
    where
        F: Fn(&str) -> Option<String>,
    {
        if let Some(var) = override_env {
            if let Some(t) = getenv(var) {
                if !t.is_empty() {
                    return Some(t);
                }
            }
        }
        for var in &["HERMES_GITHUB_TOKEN", "GITHUB_TOKEN"] {
            if let Some(t) = getenv(var) {
                if !t.is_empty() {
                    return Some(t);
                }
            }
        }
        None
    }

    /// Create an anonymous (no-token) auth.
    pub fn anonymous() -> Self {
        Self { token: None }
    }

    /// Create auth from an explicit token string.
    pub fn from_token(token: String) -> Self {
        Self { token: Some(token) }
    }

    /// Returns `Some("Bearer <token>")` if a token is present, `None` otherwise.
    pub fn authorization_header(&self) -> Option<String> {
        self.token.as_ref().map(|t| format!("Bearer {t}"))
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub fn is_anonymous(&self) -> bool {
        self.token.is_none()
    }
}

impl fmt::Debug for GitHubAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitHubAuth")
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_from_env_override_precedence() {
        let env = |k: &str| match k {
            "MY_OVERRIDE" => Some("override-token".to_string()),
            "HERMES_GITHUB_TOKEN" => Some("hermes-token".to_string()),
            "GITHUB_TOKEN" => Some("github-token".to_string()),
            _ => None,
        };
        assert_eq!(
            GitHubAuth::resolve_from_env(Some("MY_OVERRIDE"), env),
            Some("override-token".to_string())
        );
    }

    #[test]
    fn test_resolve_from_env_hermes_then_github() {
        let env_hermes = |k: &str| match k {
            "HERMES_GITHUB_TOKEN" => Some("hermes-token".to_string()),
            "GITHUB_TOKEN" => Some("github-token".to_string()),
            _ => None,
        };
        assert_eq!(
            GitHubAuth::resolve_from_env(None, env_hermes),
            Some("hermes-token".to_string())
        );

        let env_github = |k: &str| match k {
            "GITHUB_TOKEN" => Some("github-token".to_string()),
            _ => None,
        };
        assert_eq!(
            GitHubAuth::resolve_from_env(None, env_github),
            Some("github-token".to_string())
        );
    }

    #[test]
    fn test_resolve_from_env_none_when_unset() {
        let env = |_: &str| None;
        assert_eq!(GitHubAuth::resolve_from_env(None, env), None);
        assert_eq!(GitHubAuth::resolve_from_env(Some("ABSENT"), env), None);
    }

    #[test]
    fn test_debug_redacts_token() {
        let auth = GitHubAuth {
            token: Some("secret-xyz".to_string()),
        };
        let dbg = format!("{:?}", auth);
        assert!(
            !dbg.contains("secret-xyz"),
            "token must not leak in Debug: {}",
            dbg
        );
        assert!(dbg.contains("<redacted>"));
    }

    #[test]
    fn test_debug_none_token() {
        let auth = GitHubAuth { token: None };
        let dbg = format!("{:?}", auth);
        assert!(!dbg.contains("<redacted>"));
        assert!(dbg.contains("None"));
    }
}
