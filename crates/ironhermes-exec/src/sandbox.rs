/// Sandbox child-process orchestration with env stripping and timeout.

/// Result of a sandboxed Python script execution.
#[derive(Debug, Clone)]
pub struct SandboxResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// Sandbox that spawns Python child processes in an isolated environment.
pub struct Sandbox;
