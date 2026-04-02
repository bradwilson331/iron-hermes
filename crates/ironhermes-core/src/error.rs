use thiserror::Error;

#[derive(Error, Debug)]
pub enum HermesError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("API error: {0}")]
    Api(String),

    #[error("Tool error: {0}")]
    Tool(String),

    #[error("State error: {0}")]
    State(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Context window exceeded: used {used} of {limit} tokens")]
    ContextOverflow { used: usize, limit: usize },

    #[error("Max iterations reached: {0}")]
    MaxIterations(usize),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, HermesError>;
