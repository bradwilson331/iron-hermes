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
