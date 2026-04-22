use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HubErrorKind {
    RateLimited,
    Network,
    NotFound,
    AuthRequired,
    TrustRejected,
    ScanBlocked,
    AlreadyInstalled,
    InvalidIdentifier,
    Io,
    Parse,
    Internal,
    // Phase 21.8 additions per D-24.
    /// Local folder SHA-256 does not match the `computedHash` stored in
    /// `skills-lock.json`. Used by `list` / `update` to detect filesystem
    /// tampering. Emitted as a warning on `list` per D-13; as a pre-update
    /// check on `update`. NOT a server/client parity check — `snapshotHash`
    /// (D-14) is opaque and never recomputed client-side.
    ShaMismatch,
    /// Community-trust skill with scan pattern hit; differs semantically
    /// from `ScanBlocked` (which is the 19.1 trust-gate rejection). Keep
    /// both; the new blob pipeline emits `ScanHit`.
    ScanHit,
    /// `sanitize_subpath` / `assert_temp_contained` / `is_path_safe`
    /// violations.
    PathTraversal,
    /// Reserved for internal audit module error plumbing (D-19 is
    /// soft-fail, so this variant rarely surfaces to callers).
    Audit,
}

#[derive(Debug, Error)]
pub enum HubError {
    #[error("hub error ({kind:?}): {message}")]
    Typed {
        kind: HubErrorKind,
        message: String,
        suggestion: Option<String>,
        retry_after_s: Option<u64>,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Yaml(#[from] serde_yaml::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl HubError {
    pub fn envelope(&self) -> serde_json::Value {
        let (kind, message, suggestion, retry_after_s) = match self {
            HubError::Typed { kind, message, suggestion, retry_after_s } => (
                *kind,
                message.clone(),
                suggestion.clone(),
                *retry_after_s,
            ),
            _ => (HubErrorKind::Internal, self.to_string(), None, None),
        };
        serde_json::json!({
            "error": "hub_error",
            "kind": kind,
            "message": message,
            "suggestion": suggestion,
            "retry_after_s": retry_after_s,
        })
    }
}

#[cfg(test)]
mod tests_21_8 {
    use super::*;

    #[test]
    fn new_variants_serialize_snake_case() {
        assert_eq!(
            serde_json::to_string(&HubErrorKind::ShaMismatch).unwrap(),
            r#""sha_mismatch""#
        );
        assert_eq!(
            serde_json::to_string(&HubErrorKind::ScanHit).unwrap(),
            r#""scan_hit""#
        );
        assert_eq!(
            serde_json::to_string(&HubErrorKind::PathTraversal).unwrap(),
            r#""path_traversal""#
        );
        assert_eq!(
            serde_json::to_string(&HubErrorKind::Audit).unwrap(),
            r#""audit""#
        );
    }

    #[test]
    fn new_variants_partial_eq() {
        assert_eq!(HubErrorKind::ShaMismatch, HubErrorKind::ShaMismatch);
        assert_ne!(HubErrorKind::ShaMismatch, HubErrorKind::ScanHit);
        assert_ne!(HubErrorKind::PathTraversal, HubErrorKind::Audit);
    }

    #[test]
    fn envelope_shape_for_new_variants() {
        for kind in [
            HubErrorKind::ShaMismatch,
            HubErrorKind::ScanHit,
            HubErrorKind::PathTraversal,
            HubErrorKind::Audit,
        ] {
            let err = HubError::Typed {
                kind,
                message: "test message".to_string(),
                suggestion: Some("fix it".to_string()),
                retry_after_s: None,
            };
            let env = err.envelope();
            assert_eq!(env["error"], "hub_error");
            assert_eq!(env["message"], "test message");
            // `kind` should serialize via snake_case.
            assert!(env["kind"].is_string(), "kind must serialize as a JSON string");
        }
    }
}
